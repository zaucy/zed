// todo!("windows"): remove
#![allow(unused_variables)]

use std::{
    cell::RefCell,
    collections::HashSet,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Result};
use async_task::Runnable;
use clipboard_win::{get_clipboard_string, set_clipboard_string};
use futures::channel::oneshot::Receiver;
use parking_lot::Mutex;
use time::UtcOffset;
use util::{ResultExt, SemanticVersion};
use windows::Win32::{
    Foundation::{
        CloseHandle, GetLastError, HANDLE, HINSTANCE, HWND, LRESULT, WAIT_EVENT, WAIT_FAILED,
        WAIT_OBJECT_0,
    },
    System::{
        DataExchange::SetClipboardData,
        Threading::{CreateEventW, GetCurrentProcess, GetCurrentThread, ResetEvent, INFINITE},
    },
    UI::{
        Input::KeyboardAndMouse::GetActiveWindow,
        WindowsAndMessaging::{
            DefWindowProcW, DispatchMessageW, GetMessageW, GetWindowLongPtrW, GetWindowLongW,
            LoadCursorW, MsgWaitForMultipleObjects, PeekMessageW, PostQuitMessage, SetCursor,
            TranslateMessage, GWLP_USERDATA, IDC_ARROW, IDC_CROSS, IDC_HAND, IDC_IBEAM, IDC_NO,
            IDC_SIZENS, IDC_SIZEWE, IDC_UPARROW, MSG, PM_REMOVE, QS_ALLINPUT,
            WINDOW_LONG_PTR_INDEX, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

use crate::{
    Action, AnyWindowHandle, BackgroundExecutor, CallbackResult, ClipboardItem, CursorStyle,
    ForegroundExecutor, Keymap, Menu, PathPromptOptions, Platform, PlatformDisplay, PlatformInput,
    PlatformTextSystem, PlatformWindow, Task, WindowAppearance, WindowId, WindowOptions,
    WindowsDispatcher, WindowsDisplay, WindowsTextSystem, WindowsWindow, WindowsWindowInner,
};

pub(crate) struct WindowsPlatform {
    inner: Rc<WindowsPlatformInner>,
}

pub(crate) struct WindowsPlatformInner {
    background_executor: BackgroundExecutor,
    pub(crate) foreground_executor: ForegroundExecutor,
    main_receiver: flume::Receiver<Runnable>,
    text_system: Arc<WindowsTextSystem>,
    callbacks: Mutex<Callbacks>,
    pub(crate) window_handles: RefCell<HashSet<AnyWindowHandle>>,
    pub(crate) event: HANDLE,
}

impl Drop for WindowsPlatformInner {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.event) }.ok();
    }
}

#[derive(Default)]
struct Callbacks {
    open_urls: Option<Box<dyn FnMut(Vec<String>)>>,
    become_active: Option<Box<dyn FnMut()>>,
    resign_active: Option<Box<dyn FnMut()>>,
    quit: Option<Box<dyn FnMut()>>,
    reopen: Option<Box<dyn FnMut()>>,
    event: Option<Box<dyn FnMut(PlatformInput) -> bool>>,
    app_menu_action: Option<Box<dyn FnMut(&dyn Action)>>,
    will_open_app_menu: Option<Box<dyn FnMut()>>,
    validate_app_menu_command: Option<Box<dyn FnMut(&dyn Action) -> bool>>,
}

impl WindowsPlatform {
    pub(crate) fn new() -> Self {
        let (main_sender, main_receiver) = flume::unbounded::<Runnable>();
        let event = unsafe { CreateEventW(None, false, false, None) }.unwrap();
        let dispatcher = Arc::new(WindowsDispatcher::new(main_sender, event));
        let background_executor = BackgroundExecutor::new(dispatcher.clone());
        let foreground_executor = ForegroundExecutor::new(dispatcher);
        let text_system = Arc::new(WindowsTextSystem::new());
        let callbacks = Mutex::new(Callbacks::default());
        let window_handles = RefCell::new(HashSet::new());
        let inner = Rc::new(WindowsPlatformInner {
            background_executor,
            foreground_executor,
            main_receiver,
            text_system,
            callbacks,
            window_handles,
            event,
        });
        Self { inner }
    }

    /// returns true if message is handled and should not dispatch
    fn run_immediate_msg_handlers(&self, msg: &MSG) -> bool {
        let ptr =
            unsafe { get_window_long(msg.hwnd, GWLP_USERDATA) } as *mut Weak<WindowsWindowInner>;
        if ptr.is_null() {
            return false;
        }

        let inner = unsafe { &*ptr };
        if let Some(inner) = inner.upgrade() {
            match msg.message {
                WM_KEYDOWN | WM_SYSKEYDOWN => inner.handle_keydown_msg(msg.wParam).is_handled(),
                WM_KEYUP | WM_SYSKEYUP => inner.handle_keyup_msg(msg.wParam).is_handled(),
                _ => false,
            }
        } else {
            false
        }
    }

    fn try_get_message(&self) -> Option<MSG> {
        let mut msg = MSG::default();
        match unsafe { PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_REMOVE) }.as_bool() {
            true => Some(msg),
            false => None,
        }
    }

    fn message_loop(&self) {
        const MAX_MESSAGE_PROC: i32 = 20;

        loop {
            let wait_index = unsafe {
                MsgWaitForMultipleObjects(Some(&[self.inner.event]), false, INFINITE, QS_ALLINPUT)
            };

            if wait_index == WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
                let mut msg_proc_count = 0;
                while let Some(msg) = self.try_get_message() {
                    if msg.message == WM_QUIT {
                        return;
                    }

                    if !self.run_immediate_msg_handlers(&msg) {
                        unsafe { TranslateMessage(&msg) };
                    }

                    unsafe { DispatchMessageW(&msg) };

                    msg_proc_count += 1;

                    if msg_proc_count > MAX_MESSAGE_PROC {
                        break;
                    }
                }
            }

            std::debug_assert_ne!(wait_index, WAIT_FAILED);

            for runnable in self.inner.main_receiver.drain() {
                runnable.run();
            }
        }
    }
}

pub(crate) unsafe fn get_window_long(hwnd: HWND, nindex: WINDOW_LONG_PTR_INDEX) -> isize {
    #[cfg(target_pointer_width = "64")]
    unsafe {
        GetWindowLongPtrW(hwnd, nindex)
    }
    #[cfg(target_pointer_width = "32")]
    unsafe {
        GetWindowLongW(hwnd, nindex) as isize
    }
}

impl Platform for WindowsPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.inner.background_executor.clone()
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        self.inner.foreground_executor.clone()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.inner.text_system.clone()
    }

    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        on_finish_launching();
        self.message_loop();
        let mut callbacks = self.inner.callbacks.lock();
        if let Some(callback) = callbacks.quit.as_mut() {
            callback()
        }
    }

    fn quit(&self) {
        self.foreground_executor()
            .spawn(async { unsafe { PostQuitMessage(0) } })
            .detach();
    }

    // todo!("windows")
    fn restart(&self) {
        unimplemented!()
    }

    // todo!("windows")
    fn activate(&self, ignoring_other_apps: bool) {}

    // todo!("windows")
    fn hide(&self) {
        unimplemented!()
    }

    // todo!("windows")
    fn hide_other_apps(&self) {
        unimplemented!()
    }

    // todo!("windows")
    fn unhide_other_apps(&self) {
        unimplemented!()
    }

    // todo!("windows")
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        vec![Rc::new(WindowsDisplay::new())]
    }

    // todo!("windows")
    fn display(&self, id: crate::DisplayId) -> Option<Rc<dyn PlatformDisplay>> {
        Some(Rc::new(WindowsDisplay::new()))
    }

    // todo!("windows")
    fn active_window(&self) -> Option<AnyWindowHandle> {
        let active_hwnd = unsafe { GetActiveWindow() };
        if active_hwnd.0 == 0 {
            return None;
        }

        let ptr =
            unsafe { get_window_long(active_hwnd, GWLP_USERDATA) } as *mut Weak<WindowsWindowInner>;
        if ptr.is_null() {
            return None;
        }

        let inner = unsafe { &*ptr };
        if let Some(inner) = inner.upgrade() {
            return Some(inner.handle);
        }

        return None;
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowOptions,
    ) -> Box<dyn PlatformWindow> {
        Box::new(WindowsWindow::new(self.inner.clone(), handle, options))
    }

    // todo!("windows")
    fn window_appearance(&self) -> WindowAppearance {
        WindowAppearance::Dark
    }

    // todo!("windows")
    fn open_url(&self, url: &str) {
        // todo!("windows")
    }

    // todo!("windows")
    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        self.inner.callbacks.lock().open_urls = Some(callback);
    }

    // todo!("windows")
    fn prompt_for_paths(&self, options: PathPromptOptions) -> Receiver<Option<Vec<PathBuf>>> {
        unimplemented!()
    }

    // todo!("windows")
    fn prompt_for_new_path(&self, directory: &Path) -> Receiver<Option<PathBuf>> {
        unimplemented!()
    }

    // todo!("windows")
    fn reveal_path(&self, path: &Path) {
        unimplemented!()
    }

    fn on_become_active(&self, callback: Box<dyn FnMut()>) {
        self.inner.callbacks.lock().become_active = Some(callback);
    }

    fn on_resign_active(&self, callback: Box<dyn FnMut()>) {
        self.inner.callbacks.lock().resign_active = Some(callback);
    }

    fn on_quit(&self, callback: Box<dyn FnMut()>) {
        self.inner.callbacks.lock().quit = Some(callback);
    }

    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        self.inner.callbacks.lock().reopen = Some(callback);
    }

    fn on_event(&self, callback: Box<dyn FnMut(PlatformInput) -> bool>) {
        self.inner.callbacks.lock().event = Some(callback);
    }

    // todo!("windows")
    fn set_menus(&self, menus: Vec<Menu>, keymap: &Keymap) {}

    fn on_app_menu_action(&self, callback: Box<dyn FnMut(&dyn Action)>) {
        self.inner.callbacks.lock().app_menu_action = Some(callback);
    }

    fn on_will_open_app_menu(&self, callback: Box<dyn FnMut()>) {
        self.inner.callbacks.lock().will_open_app_menu = Some(callback);
    }

    fn on_validate_app_menu_command(&self, callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        self.inner.callbacks.lock().validate_app_menu_command = Some(callback);
    }

    fn os_name(&self) -> &'static str {
        "Windows"
    }

    fn os_version(&self) -> Result<SemanticVersion> {
        Ok(SemanticVersion {
            major: 1,
            minor: 0,
            patch: 0,
        })
    }

    fn app_version(&self) -> Result<SemanticVersion> {
        Ok(SemanticVersion {
            major: 1,
            minor: 0,
            patch: 0,
        })
    }

    // todo!("windows")
    fn app_path(&self) -> Result<PathBuf> {
        Err(anyhow!("not yet implemented"))
    }

    // todo!("windows")
    fn local_timezone(&self) -> UtcOffset {
        UtcOffset::from_hms(9, 0, 0).unwrap()
    }

    // todo!("windows")
    fn double_click_interval(&self) -> Duration {
        Duration::from_millis(100)
    }

    // todo!("windows")
    fn path_for_auxiliary_executable(&self, name: &str) -> Result<PathBuf> {
        Err(anyhow!("not yet implemented"))
    }

    // todo!("windows")
    fn set_cursor_style(&self, style: CursorStyle) {
        let win_cursor_name = match style {
            CursorStyle::Arrow => IDC_ARROW,
            CursorStyle::IBeam => IDC_IBEAM,
            CursorStyle::Crosshair => IDC_CROSS,
            CursorStyle::ClosedHand => IDC_ARROW,
            CursorStyle::OpenHand => IDC_ARROW,
            CursorStyle::PointingHand => IDC_HAND,
            CursorStyle::ResizeLeft => IDC_SIZEWE,
            CursorStyle::ResizeRight => IDC_SIZEWE,
            CursorStyle::ResizeLeftRight => IDC_SIZEWE,
            CursorStyle::ResizeUp => IDC_SIZENS,
            CursorStyle::ResizeDown => IDC_SIZENS,
            CursorStyle::ResizeUpDown => IDC_SIZENS,
            CursorStyle::DisappearingItem => IDC_UPARROW,
            CursorStyle::IBeamCursorForVerticalLayout => IDC_IBEAM,
            CursorStyle::OperationNotAllowed => IDC_NO,
            CursorStyle::DragLink => IDC_ARROW,
            CursorStyle::DragCopy => IDC_ARROW,
            CursorStyle::ContextualMenu => IDC_ARROW,
        };

        let cursor = unsafe { LoadCursorW(HINSTANCE::default(), win_cursor_name) }.log_err();

        if let Some(cursor) = cursor {
            unsafe { SetCursor(cursor) };
        } else {
            log::error!("Failed to set cursor");
        }
    }

    // todo!("windows")
    fn should_auto_hide_scrollbars(&self) -> bool {
        false
    }

    // todo!("windows")
    fn write_to_clipboard(&self, item: ClipboardItem) {
        if let Err(err) = set_clipboard_string(item.text().as_str()) {
            log::error!("Clipboard: {}", err.to_string());
        }
    }

    // todo!("windows")
    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        get_clipboard_string()
            .ok()
            .map(|content| ClipboardItem::new(content))
    }

    // todo!("windows")
    fn write_credentials(&self, url: &str, username: &str, password: &[u8]) -> Task<Result<()>> {
        Task::Ready(Some(Err(anyhow!("write_credentials not implemented yet."))))
    }

    // todo!("windows")
    fn read_credentials(&self, url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::Ready(Some(Err(anyhow!("read_credentials not implemented yet."))))
    }

    // todo!("windows")
    fn delete_credentials(&self, url: &str) -> Task<Result<()>> {
        Task::Ready(Some(Err(anyhow!(
            "delete_credentials not implemented yet."
        ))))
    }
}
