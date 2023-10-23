use crate::Project;
use anyhow::Context;
use client::Client;
use gpui::{
    AnyWindowHandle, AppContext, AsyncAppContext, ModelContext, ModelHandle, WeakModelHandle,
};
use rpc::{
    proto::{self, OpenTerminal, OpenTerminalResponse, UpdateTerminals},
    TypedEnvelope,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use terminal::{
    terminal_settings::{self, TerminalSettings, VenvSettingsContent},
    Terminal, TerminalBuilder,
};
use util::ResultExt;

#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;

pub type TerminalId = u64;

#[derive(Default)]
pub struct Terminals {
    pub(crate) local_handles: Vec<WeakModelHandle<terminal::Terminal>>,
    pub(crate) remote_ids: Vec<TerminalId>,
}

impl Project {
    pub fn init_terminals(client: &Arc<Client>, _: &mut AppContext) {
        client.add_model_message_handler(Self::handle_update_terminals);
        client.add_model_request_handler(Self::handle_open_terminal);
    }

    pub fn shared_terminals(&self) -> &[TerminalId] {
        &self.terminals.remote_ids
    }

    pub async fn handle_update_terminals(
        this: ModelHandle<Self>,
        envelope: TypedEnvelope<proto::UpdateTerminals>,
        _: Arc<Client>,
        mut cx: AsyncAppContext,
    ) -> anyhow::Result<()> {
        this.update(&mut cx, |this, cx| {
            this.terminals.remote_ids = envelope.payload.terminals;
            cx.notify();
        });
        Ok(())
    }

    pub async fn handle_open_terminal(
        this: ModelHandle<Self>,
        envelope: TypedEnvelope<proto::OpenTerminal>,
        _: Arc<Client>,
        cx: AsyncAppContext,
    ) -> anyhow::Result<OpenTerminalResponse> {
        let terminal_id = envelope.payload.id as usize;
        let terminal = this
            .read_with(&cx, |this, cx| {
                this.terminals.local_handles.iter().find_map(|handle| {
                    if handle.id() == terminal_id {
                        Some(handle.upgrade(cx)?)
                    } else {
                        None
                    }
                })
            })
            .with_context(|| format!("no terminal found for {terminal_id}"))?;

        terminal.read_with(&cx, |terminal, cx| {
            // 1. We need to make sure we have synchronized with the terminal, before we do this
            // 2. We need to pull both sets of state out of the event loop.
        });

        Ok(OpenTerminalResponse {
            vte_state: todo!("TODO kb"),
            visible_terminal_cells: todo!(),
        })
    }

    pub fn open_remote_terminal(
        &mut self,
        project_id: u64,
        remote_terminal_id: TerminalId,
        window: AnyWindowHandle,
        cx: &mut ModelContext<Self>,
    ) -> anyhow::Result<ModelHandle<Terminal>> {
        let z = self
            .client
            .send(OpenTerminal {
                project_id,
                id: remote_terminal_id,
            })
            .context("remote terminal creation message send")?;
        todo!("TODO kb");
    }

    pub fn create_terminal(
        &mut self,
        working_directory: Option<PathBuf>,
        window: AnyWindowHandle,
        cx: &mut ModelContext<Self>,
    ) -> anyhow::Result<ModelHandle<Terminal>> {
        if self.is_remote() {
            return Err(anyhow::anyhow!(
                "creating terminals as a guest is not supported yet"
            ));
        } else {
            let settings = settings::get::<TerminalSettings>(cx);
            let python_settings = settings.detect_venv.clone();
            let shell = settings.shell.clone();
            TerminalBuilder::new(
                working_directory.clone(),
                shell.clone(),
                settings.env.clone(),
                Some(settings.blinking.clone()),
                settings.alternate_scroll,
                window,
                self.client(),
            )
            .map(|builder| {
                let terminal_handle = cx.add_model(|cx| builder.subscribe(cx));

                self.terminals
                    .local_handles
                    .push(terminal_handle.downgrade());

                let id = terminal_handle.id();
                cx.observe_release(&terminal_handle, move |project, _terminal, cx| {
                    let handles = &mut project.terminals.local_handles;

                    if let Some(index) = handles.iter().position(|terminal| terminal.id() == id) {
                        handles.remove(index);
                        cx.notify();
                    }
                })
                .detach();

                if let Some(python_settings) = &python_settings.as_option() {
                    let activate_script_path =
                        self.find_activate_script_path(&python_settings, working_directory);
                    self.activate_python_virtual_environment(
                        activate_script_path,
                        &terminal_handle,
                        cx,
                    );
                }

                if let Some(project_id) = self.remote_id() {
                    self.client
                        .send(UpdateTerminals {
                            project_id,
                            terminals: self
                                .terminals
                                .local_handles
                                .iter()
                                .map(|handle| handle.id() as TerminalId)
                                .collect(),
                        })
                        .log_err();
                }

                terminal_handle
            })
        }
    }

    pub fn find_activate_script_path(
        &mut self,
        settings: &VenvSettingsContent,
        working_directory: Option<PathBuf>,
    ) -> Option<PathBuf> {
        // When we are unable to resolve the working directory, the terminal builder
        // defaults to '/'. We should probably encode this directly somewhere, but for
        // now, let's just hard code it here.
        let working_directory = working_directory.unwrap_or_else(|| Path::new("/").to_path_buf());
        let activate_script_name = match settings.activate_script {
            terminal_settings::ActivateScript::Default => "activate",
            terminal_settings::ActivateScript::Csh => "activate.csh",
            terminal_settings::ActivateScript::Fish => "activate.fish",
            terminal_settings::ActivateScript::Nushell => "activate.nu",
        };

        for virtual_environment_name in settings.directories {
            let mut path = working_directory.join(virtual_environment_name);
            path.push("bin/");
            path.push(activate_script_name);

            if path.exists() {
                return Some(path);
            }
        }

        None
    }

    fn activate_python_virtual_environment(
        &mut self,
        activate_script: Option<PathBuf>,
        terminal_handle: &ModelHandle<Terminal>,
        cx: &mut ModelContext<Project>,
    ) {
        if let Some(activate_script) = activate_script {
            // Paths are not strings so we need to jump through some hoops to format the command without `format!`
            let mut command = Vec::from("source ".as_bytes());
            command.extend_from_slice(activate_script.as_os_str().as_bytes());
            command.push(b'\n');

            terminal_handle.update(cx, |this, _| this.input_bytes(command));
        }
    }

    pub fn local_terminal_handles(&self) -> &[WeakModelHandle<Terminal>] {
        &self.terminals.local_handles
    }
}

// TODO: Add a few tests for adding and removing terminal tabs
