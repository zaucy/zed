#![deny(unsafe_op_in_unsafe_fn)]
// todo!("windows"): remove
#![allow(unused_variables)]

use std::{
    any::Any,
    cell::{Cell, RefCell},
    ffi::c_void,
    num::NonZeroIsize,
    rc::{Rc, Weak},
    sync::{Arc, Once},
};

use blade_graphics as gpu;
use futures::channel::oneshot::Receiver;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::SystemServices::{
            MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MK_XBUTTON1, MK_XBUTTON2, MODIFIERKEYS_FLAGS,
        },
        UI::{
            Input::KeyboardAndMouse::{
                GetKeyState, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_F1,
                VK_F24, VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR,
                VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, LoadCursorW, PostQuitMessage, RegisterClassW,
                SetWindowLongPtrW, SetWindowLongW, SetWindowTextW, ShowWindow, CREATESTRUCTW,
                CW_USEDEFAULT, GWLP_USERDATA, HMENU, IDC_ARROW, SW_MAXIMIZE, SW_SHOW,
                WINDOW_EX_STYLE, WINDOW_LONG_PTR_INDEX, WM_CHAR, WM_CLOSE, WM_DESTROY, WM_KEYDOWN,
                WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
                WM_MOVE, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP,
                WM_SIZE, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
                WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE, XBUTTON1, XBUTTON2,
            },
        },
    },
};

use crate::{
    get_window_long, platform::blade::BladeRenderer, AnyWindowHandle, Bounds, GlobalPixels,
    HiLoWord, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels, PlatformAtlas, PlatformDisplay,
    PlatformInput, PlatformInputHandler, PlatformWindow, Point, PromptLevel, Scene, Size,
    WindowAppearance, WindowBounds, WindowOptions, WindowsDisplay, WindowsPlatformInner,
};

pub(crate) struct WindowsWindowInner {
    hwnd: HWND,
    origin: Cell<Point<GlobalPixels>>,
    size: Cell<Size<GlobalPixels>>,
    mouse_position: Cell<Point<Pixels>>,
    input_handler: Cell<Option<PlatformInputHandler>>,
    renderer: RefCell<BladeRenderer>,
    callbacks: RefCell<Callbacks>,
    platform_inner: Rc<WindowsPlatformInner>,
    pub(crate) handle: AnyWindowHandle,
}

#[derive(PartialEq)]
pub(crate) enum CallbackResult {
    Handled(bool),
    Unhandled,
}

impl CallbackResult {
    pub fn is_handled(&self) -> bool {
        match self {
            Self::Handled(true) => true,
            Self::Handled(false) => true,
            _ => false,
        }
    }
}

impl WindowsWindowInner {
    fn new(
        hwnd: HWND,
        cs: &CREATESTRUCTW,
        platform_inner: Rc<WindowsPlatformInner>,
        handle: AnyWindowHandle,
    ) -> Self {
        let origin = Cell::new(Point::new((cs.x as f64).into(), (cs.y as f64).into()));
        let size = Cell::new(Size {
            width: (cs.cx as f64).into(),
            height: (cs.cy as f64).into(),
        });
        let mouse_position = Cell::new(Point::default());
        let input_handler = Cell::new(None);
        struct RawWindow {
            hwnd: *mut c_void,
        }
        unsafe impl blade_rwh::HasRawWindowHandle for RawWindow {
            fn raw_window_handle(&self) -> blade_rwh::RawWindowHandle {
                let mut handle = blade_rwh::Win32WindowHandle::empty();
                handle.hwnd = self.hwnd;
                handle.into()
            }
        }
        unsafe impl blade_rwh::HasRawDisplayHandle for RawWindow {
            fn raw_display_handle(&self) -> blade_rwh::RawDisplayHandle {
                blade_rwh::WindowsDisplayHandle::empty().into()
            }
        }
        let raw = RawWindow { hwnd: hwnd.0 as _ };
        let gpu = Arc::new(
            unsafe {
                gpu::Context::init_windowed(
                    &raw,
                    gpu::ContextDesc {
                        validation: false,
                        capture: false,
                    },
                )
            }
            .unwrap(),
        );
        let extent = gpu::Extent {
            width: 1,
            height: 1,
            depth: 1,
        };
        let renderer = RefCell::new(BladeRenderer::new(gpu, extent));
        let callbacks = RefCell::new(Callbacks::default());
        Self {
            hwnd,
            origin,
            size,
            mouse_position,
            input_handler,
            renderer,
            callbacks,
            platform_inner,
            handle,
        }
    }

    fn is_virtual_key_pressed(&self, vkey: VIRTUAL_KEY) -> bool {
        unsafe { GetKeyState(vkey.0 as i32) < 0 }
    }

    fn current_modifiers(&self) -> Modifiers {
        Modifiers {
            control: self.is_virtual_key_pressed(VK_CONTROL),
            alt: self.is_virtual_key_pressed(VK_MENU),
            shift: self.is_virtual_key_pressed(VK_SHIFT),
            command: self.is_virtual_key_pressed(VK_LWIN) || self.is_virtual_key_pressed(VK_RWIN),
            function: false,
        }
    }

    fn handle_msg(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        log::debug!("msg: {msg}, wparam: {}, lparam: {}", wparam.0, lparam.0);
        match msg {
            WM_MOVE => {
                let x = lparam.loword() as f64;
                let y = lparam.hiword() as f64;
                self.origin.set(Point::new(x.into(), y.into()));
                let mut callbacks = self.callbacks.borrow_mut();
                if let Some(callback) = callbacks.moved.as_mut() {
                    callback()
                }
            }
            WM_SIZE => {
                // todo!("windows"): handle maximized or minimized
                let width = lparam.loword().max(1) as f64;
                let height = lparam.hiword().max(1) as f64;
                self.renderer
                    .borrow_mut()
                    .update_drawable_size(Size { width, height });
                let width = width.into();
                let height = height.into();
                self.size.set(Size { width, height });
                let mut callbacks = self.callbacks.borrow_mut();
                if let Some(callback) = callbacks.resize.as_mut() {
                    callback(
                        Size {
                            width: Pixels(width.0),
                            height: Pixels(height.0),
                        },
                        1.0,
                    )
                }
            }
            WM_PAINT => {
                let mut callbacks = self.callbacks.borrow_mut();
                if let Some(callback) = callbacks.request_frame.as_mut() {
                    callback()
                }
            }
            WM_CLOSE => {
                let mut callbacks: std::cell::RefMut<'_, Callbacks> = self.callbacks.borrow_mut();
                if let Some(callback) = callbacks.should_close.as_mut() {
                    if callback() {
                        return LRESULT(0);
                    }
                }
                drop(callbacks);
                return unsafe { DefWindowProcW(self.hwnd, msg, wparam, lparam) };
            }
            WM_DESTROY => {
                let mut callbacks: std::cell::RefMut<'_, Callbacks> = self.callbacks.borrow_mut();
                if let Some(callback) = callbacks.close.take() {
                    callback()
                }
                let mut window_handles = self.platform_inner.window_handles.borrow_mut();
                window_handles.remove(&self.handle);
                if window_handles.is_empty() {
                    self.platform_inner
                        .foreground_executor
                        .spawn(async {
                            unsafe { PostQuitMessage(0) };
                        })
                        .detach();
                }
                return LRESULT(1);
            }
            WM_MOUSEMOVE => {
                let x = Pixels::from(lparam.loword() as f32);
                let y = Pixels::from(lparam.hiword() as f32);
                self.mouse_position.set(Point { x, y });
                let mut callbacks: std::cell::RefMut<'_, Callbacks> = self.callbacks.borrow_mut();
                if let Some(callback) = callbacks.input.as_mut() {
                    let pressed_button = match MODIFIERKEYS_FLAGS(wparam.loword() as u32) {
                        flags if flags.contains(MK_LBUTTON) => Some(MouseButton::Left),
                        flags if flags.contains(MK_RBUTTON) => Some(MouseButton::Right),
                        flags if flags.contains(MK_MBUTTON) => Some(MouseButton::Middle),
                        flags if flags.contains(MK_XBUTTON1) => {
                            Some(MouseButton::Navigate(NavigationDirection::Back))
                        }
                        flags if flags.contains(MK_XBUTTON2) => {
                            Some(MouseButton::Navigate(NavigationDirection::Forward))
                        }
                        _ => None,
                    };
                    let event = MouseMoveEvent {
                        position: Point { x, y },
                        pressed_button,
                        modifiers: self.current_modifiers(),
                    };
                    if callback(PlatformInput::MouseMove(event)) {
                        return LRESULT(0);
                    }
                }
                return LRESULT(1);
            }
            WM_LBUTTONDOWN => {
                return self.handle_mouse_down_msg(MouseButton::Left, lparam);
            }
            WM_RBUTTONDOWN => {
                return self.handle_mouse_down_msg(MouseButton::Right, lparam);
            }
            WM_MBUTTONDOWN => {
                return self.handle_mouse_down_msg(MouseButton::Middle, lparam);
            }
            WM_XBUTTONDOWN => {
                let nav_dir = match wparam.hiword() {
                    XBUTTON1 => Some(NavigationDirection::Forward),
                    XBUTTON2 => Some(NavigationDirection::Back),
                    _ => None,
                };

                if let Some(nav_dir) = nav_dir {
                    return self.handle_mouse_down_msg(MouseButton::Navigate(nav_dir), lparam);
                }
                return LRESULT(1);
            }
            WM_LBUTTONUP => {
                return self.handle_mouse_up_msg(MouseButton::Left, lparam);
            }
            WM_RBUTTONUP => {
                return self.handle_mouse_up_msg(MouseButton::Right, lparam);
            }
            WM_MBUTTONUP => {
                return self.handle_mouse_up_msg(MouseButton::Middle, lparam);
            }
            WM_XBUTTONUP => {
                let nav_dir = match wparam.hiword() {
                    XBUTTON1 => Some(NavigationDirection::Forward),
                    XBUTTON2 => Some(NavigationDirection::Back),
                    _ => None,
                };

                if let Some(nav_dir) = nav_dir {
                    return self.handle_mouse_up_msg(MouseButton::Navigate(nav_dir), lparam);
                }
                return LRESULT(1);
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                return LRESULT(0);
            }
            WM_KEYUP | WM_SYSKEYUP => {
                return LRESULT(0);
            }
            WM_CHAR | WM_SYSCHAR => {
                return self.handle_char_msg(wparam);
            }
            _ => return unsafe { DefWindowProcW(self.hwnd, msg, wparam, lparam) },
        }
        LRESULT(0)
    }

    pub(crate) fn handle_keydown_msg(&self, wparam: WPARAM) -> CallbackResult {
        let mut callbacks: std::cell::RefMut<'_, Callbacks> = self.callbacks.borrow_mut();
        let keystroke = self.parse_key_msg_keystroke(wparam);
        if let Some(keystroke) = keystroke {
            if let Some(callback) = callbacks.input.as_mut() {
                let event = KeyDownEvent {
                    keystroke,
                    is_held: true,
                };

                if callback(PlatformInput::KeyDown(event.clone())) {
                    CallbackResult::Handled(true)
                } else if let Some(mut input_handler) = self.input_handler.take() {
                    if let Some(ime_key) = &event.keystroke.ime_key {
                        input_handler.replace_text_in_range(None, ime_key);
                    }
                    self.input_handler.set(Some(input_handler));
                    CallbackResult::Handled(true)
                } else {
                    CallbackResult::Handled(false)
                }
            } else {
                CallbackResult::Handled(false)
            }
        } else {
            CallbackResult::Unhandled
        }
    }

    fn parse_key_msg_keystroke(&self, wparam: WPARAM) -> Option<Keystroke> {
        let vk_code = wparam.loword();

        // 0-9 https://learn.microsoft.com/en-us/windows/win32/inputdev/virtual-key-codes
        if vk_code >= 0x30 && vk_code <= 0x39 {
            let modifiers = self.current_modifiers();

            if modifiers.shift {
                return None;
            }

            let digit_char = ('0' as u8 + ((vk_code - 0x30) as u8)) as char;
            return Some(Keystroke {
                modifiers,
                key: digit_char.to_string(),
                ime_key: Some(digit_char.to_string()),
            });
        }

        // A-Z https://learn.microsoft.com/en-us/windows/win32/inputdev/virtual-key-codes
        if vk_code >= 0x41 && vk_code <= 0x5A {
            let offset = (vk_code - 0x41) as u8;
            let alpha_char = ('a' as u8 + offset) as char;
            let alpha_char_upper = ('A' as u8 + offset) as char;
            let modifiers = self.current_modifiers();
            return Some(Keystroke {
                modifiers,
                key: alpha_char.to_string(),
                ime_key: Some(if modifiers.shift {
                    alpha_char_upper.to_string()
                } else {
                    alpha_char.to_string()
                }),
            });
        }

        if vk_code >= VK_F1.0 && vk_code <= VK_F24.0 {
            let offset = vk_code - VK_F1.0;
            return Some(Keystroke {
                modifiers: self.current_modifiers(),
                key: format!("f{}", offset),
                ime_key: None,
            });
        }

        let key = match VIRTUAL_KEY(vk_code) {
            VK_SPACE => Some(("space", Some(" "))),
            VK_TAB => Some(("tab", Some("\t"))),
            VK_BACK => Some(("backspace", None)),
            VK_RETURN => Some(("enter", None)),
            VK_UP => Some(("up", None)),
            VK_DOWN => Some(("down", None)),
            VK_RIGHT => Some(("right", None)),
            VK_LEFT => Some(("left", None)),
            VK_HOME => Some(("home", None)),
            VK_END => Some(("end", None)),
            VK_PRIOR => Some(("pageup", None)),
            VK_NEXT => Some(("pagedown", None)),
            VK_ESCAPE => Some(("escape", None)),
            VK_INSERT => Some(("insert", None)),
            _ => None,
        };

        if let Some((key, ime_key)) = key {
            Some(Keystroke {
                modifiers: self.current_modifiers(),
                key: key.to_string(),
                ime_key: ime_key.map(|k| k.to_string()),
            })
        } else {
            None
        }
    }

    pub(crate) fn handle_keyup_msg(&self, wparam: WPARAM) -> CallbackResult {
        let mut callbacks: std::cell::RefMut<'_, Callbacks> = self.callbacks.borrow_mut();
        let keystroke = self.parse_key_msg_keystroke(wparam);
        if let Some(keystroke) = keystroke {
            if let Some(callback) = callbacks.input.as_mut() {
                let event = KeyUpEvent { keystroke };
                CallbackResult::Handled(callback(PlatformInput::KeyUp(event)))
            } else {
                CallbackResult::Handled(false)
            }
        } else {
            CallbackResult::Unhandled
        }
    }

    fn handle_char_msg(&self, wparam: WPARAM) -> LRESULT {
        let mut callbacks: std::cell::RefMut<'_, Callbacks> = self.callbacks.borrow_mut();
        if let Some(callback) = callbacks.input.as_mut() {
            let modifiers = self.current_modifiers();
            let msg_char = wparam.0 as u8 as char;
            dbg!(&msg_char);
            let keystroke = Keystroke {
                modifiers,
                key: msg_char.to_string(),
                ime_key: Some(msg_char.to_string()),
            };
            let event = KeyDownEvent {
                keystroke,
                is_held: false,
            };

            if callback(PlatformInput::KeyDown(event.clone())) {
                return LRESULT(0);
            }

            if let Some(mut input_handler) = self.input_handler.take() {
                if let Some(ime_key) = &event.keystroke.ime_key {
                    input_handler.replace_text_in_range(None, ime_key);
                }
                self.input_handler.set(Some(input_handler));
                return LRESULT(0);
            }
        }
        return LRESULT(1);
    }

    fn handle_mouse_down_msg(&self, button: MouseButton, lparam: LPARAM) -> LRESULT {
        let mut callbacks: std::cell::RefMut<'_, Callbacks> = self.callbacks.borrow_mut();
        if let Some(callback) = callbacks.input.as_mut() {
            let x = Pixels::from(lparam.loword() as f32);
            let y = Pixels::from(lparam.hiword() as f32);
            let event = MouseDownEvent {
                button,
                position: Point { x, y },
                modifiers: self.current_modifiers(),
                click_count: 1,
            };
            if callback(PlatformInput::MouseDown(event)) {
                return LRESULT(0);
            }
        }
        LRESULT(1)
    }

    fn handle_mouse_up_msg(&self, button: MouseButton, lparam: LPARAM) -> LRESULT {
        let mut callbacks: std::cell::RefMut<'_, Callbacks> = self.callbacks.borrow_mut();
        if let Some(callback) = callbacks.input.as_mut() {
            let x = Pixels::from(lparam.loword() as f32);
            let y = Pixels::from(lparam.hiword() as f32);
            let event = MouseUpEvent {
                button,
                position: Point { x, y },
                modifiers: self.current_modifiers(),
                click_count: 1,
            };
            if callback(PlatformInput::MouseUp(event)) {
                return LRESULT(0);
            }
        }
        LRESULT(1)
    }
}

fn parse_mouse_event_button(wparam: WPARAM) -> MouseButton {
    dbg!(&wparam);
    match MODIFIERKEYS_FLAGS(wparam.0 as u32) {
        flags if flags.contains(MK_LBUTTON) => MouseButton::Left,
        flags if flags.contains(MK_RBUTTON) => MouseButton::Right,
        flags if flags.contains(MK_MBUTTON) => MouseButton::Middle,
        flags if flags.contains(MK_XBUTTON1) => MouseButton::Navigate(NavigationDirection::Back),
        flags if flags.contains(MK_XBUTTON2) => MouseButton::Navigate(NavigationDirection::Forward),
        _ => panic!("Unexpected mouse wparam"),
    }
}

#[derive(Default)]
struct Callbacks {
    request_frame: Option<Box<dyn FnMut()>>,
    input: Option<Box<dyn FnMut(crate::PlatformInput) -> bool>>,
    active_status_change: Option<Box<dyn FnMut(bool)>>,
    resize: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    fullscreen: Option<Box<dyn FnMut(bool)>>,
    moved: Option<Box<dyn FnMut()>>,
    should_close: Option<Box<dyn FnMut() -> bool>>,
    close: Option<Box<dyn FnOnce()>>,
    appearance_changed: Option<Box<dyn FnMut()>>,
}

pub(crate) struct WindowsWindow {
    inner: Rc<WindowsWindowInner>,
}

struct WindowCreateContext {
    inner: Option<Rc<WindowsWindowInner>>,
    platform_inner: Rc<WindowsPlatformInner>,
    handle: AnyWindowHandle,
}

impl WindowsWindow {
    pub(crate) fn new(
        platform_inner: Rc<WindowsPlatformInner>,
        handle: AnyWindowHandle,
        options: WindowOptions,
    ) -> Self {
        let dwexstyle = WINDOW_EX_STYLE::default();
        let classname = register_wnd_class();
        let windowname = HSTRING::from(
            options
                .titlebar
                .as_ref()
                .and_then(|titlebar| titlebar.title.as_ref())
                .map(|title| title.as_ref())
                .unwrap_or(""),
        );
        let dwstyle = WS_OVERLAPPEDWINDOW & !WS_VISIBLE;
        let mut x = CW_USEDEFAULT;
        let mut y = CW_USEDEFAULT;
        let mut nwidth = CW_USEDEFAULT;
        let mut nheight = CW_USEDEFAULT;
        match options.bounds {
            WindowBounds::Fullscreen => {}
            WindowBounds::Maximized => {}
            WindowBounds::Fixed(bounds) => {
                x = bounds.origin.x.0 as i32;
                y = bounds.origin.y.0 as i32;
                nwidth = bounds.size.width.0 as i32;
                nheight = bounds.size.height.0 as i32;
            }
        };
        let hwndparent = HWND::default();
        let hmenu = HMENU::default();
        let hinstance = HINSTANCE::default();
        let mut context = WindowCreateContext {
            inner: None,
            platform_inner: platform_inner.clone(),
            handle,
        };
        let lpparam = Some(&context as *const _ as *const _);
        unsafe {
            CreateWindowExW(
                dwexstyle,
                classname,
                &windowname,
                dwstyle,
                x,
                y,
                nwidth,
                nheight,
                hwndparent,
                hmenu,
                hinstance,
                lpparam,
            )
        };
        let wnd = Self {
            inner: context.inner.unwrap(),
        };
        platform_inner.window_handles.borrow_mut().insert(handle);
        match options.bounds {
            WindowBounds::Fullscreen => wnd.toggle_full_screen(),
            WindowBounds::Maximized => wnd.maximize(),
            WindowBounds::Fixed(_) => {}
        }
        unsafe { ShowWindow(wnd.inner.hwnd, SW_SHOW) };
        wnd
    }

    fn maximize(&self) {
        log::debug!("{:?}", self.inner.hwnd);
        unsafe { ShowWindow(self.inner.hwnd, SW_MAXIMIZE) };
    }
}

impl HasWindowHandle for WindowsWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let raw = raw_window_handle::Win32WindowHandle::new(unsafe {
            NonZeroIsize::new_unchecked(self.inner.hwnd.0)
        })
        .into();
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

// todo!("windows")
impl HasDisplayHandle for WindowsWindow {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        unimplemented!()
    }
}

impl PlatformWindow for WindowsWindow {
    fn bounds(&self) -> WindowBounds {
        WindowBounds::Fixed(Bounds {
            origin: self.inner.origin.get(),
            size: self.inner.size.get(),
        })
    }

    // todo!("windows")
    fn content_size(&self) -> Size<Pixels> {
        let size = self.inner.size.get();
        Size {
            width: size.width.0.into(),
            height: size.height.0.into(),
        }
    }

    // todo!("windows")
    fn scale_factor(&self) -> f32 {
        1.0
    }

    // todo!("windows")
    fn titlebar_height(&self) -> Pixels {
        20.0.into()
    }

    // todo!("windows")
    fn appearance(&self) -> WindowAppearance {
        WindowAppearance::Dark
    }

    // todo!("windows")
    fn display(&self) -> Rc<dyn PlatformDisplay> {
        Rc::new(WindowsDisplay::new())
    }

    fn mouse_position(&self) -> Point<Pixels> {
        self.inner.mouse_position.get()
    }

    // todo!("windows")
    fn modifiers(&self) -> Modifiers {
        Modifiers::none()
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    // todo!("windows")
    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.inner.input_handler.set(Some(input_handler));
    }

    // todo!("windows")
    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.inner.input_handler.take()
    }

    // todo!("windows")
    fn prompt(
        &self,
        level: PromptLevel,
        msg: &str,
        detail: Option<&str>,
        answers: &[&str],
    ) -> Receiver<usize> {
        unimplemented!()
    }

    // todo!("windows")
    fn activate(&self) {}

    // todo!("windows")
    fn set_title(&mut self, title: &str) {
        unsafe { SetWindowTextW(self.inner.hwnd, &HSTRING::from(title)) }
            .inspect_err(|e| log::error!("Set title failed: {e}"))
            .ok();
    }

    // todo!("windows")
    fn set_edited(&mut self, edited: bool) {}

    // todo!("windows")
    fn show_character_palette(&self) {}

    // todo!("windows")
    fn minimize(&self) {}

    // todo!("windows")
    fn zoom(&self) {}

    // todo!("windows")
    fn toggle_full_screen(&self) {}

    // todo!("windows")
    fn on_request_frame(&self, callback: Box<dyn FnMut()>) {
        self.inner.callbacks.borrow_mut().request_frame = Some(callback);
    }

    // todo!("windows")
    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> bool>) {
        self.inner.callbacks.borrow_mut().input = Some(callback);
    }

    // todo!("windows")
    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.inner.callbacks.borrow_mut().active_status_change = Some(callback);
    }

    // todo!("windows")
    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.inner.callbacks.borrow_mut().resize = Some(callback);
    }

    // todo!("windows")
    fn on_fullscreen(&self, callback: Box<dyn FnMut(bool)>) {
        self.inner.callbacks.borrow_mut().fullscreen = Some(callback);
    }

    // todo!("windows")
    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.inner.callbacks.borrow_mut().moved = Some(callback);
    }

    // todo!("windows")
    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.inner.callbacks.borrow_mut().should_close = Some(callback);
    }

    // todo!("windows")
    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.inner.callbacks.borrow_mut().close = Some(callback);
    }

    // todo!("windows")
    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.inner.callbacks.borrow_mut().appearance_changed = Some(callback);
    }

    // todo!("windows")
    fn is_topmost_for_position(&self, position: Point<Pixels>) -> bool {
        true
    }

    // todo!("windows")
    fn draw(&self, scene: &Scene) {
        self.inner.renderer.borrow_mut().draw(scene)
    }

    // todo!("windows")
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.inner.renderer.borrow().sprite_atlas().clone()
    }
}

fn register_wnd_class() -> PCWSTR {
    const CLASS_NAME: PCWSTR = w!("Zed::Window");

    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW).ok().unwrap() },
            lpszClassName: PCWSTR(CLASS_NAME.as_ptr()),
            ..Default::default()
        };
        unsafe { RegisterClassW(&wc) };
    });

    CLASS_NAME
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lparam.0 as *const CREATESTRUCTW;
        let cs = unsafe { &*cs };
        let ctx = cs.lpCreateParams as *mut WindowCreateContext;
        let ctx = unsafe { &mut *ctx };
        let inner = Rc::new(WindowsWindowInner::new(
            hwnd,
            cs,
            ctx.platform_inner.clone(),
            ctx.handle,
        ));
        let weak = Box::new(Rc::downgrade(&inner));
        unsafe { set_window_long(hwnd, GWLP_USERDATA, Box::into_raw(weak) as isize) };
        ctx.inner = Some(inner);
        return LRESULT(1);
    }
    let ptr = unsafe { get_window_long(hwnd, GWLP_USERDATA) } as *mut Weak<WindowsWindowInner>;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let inner = unsafe { &*ptr };
    let r = if let Some(inner) = inner.upgrade() {
        inner.handle_msg(msg, wparam, lparam)
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    };
    if msg == WM_NCDESTROY {
        unsafe { set_window_long(hwnd, GWLP_USERDATA, 0) };
        unsafe { std::mem::drop(Box::from_raw(ptr)) };
    }
    r
}

unsafe fn set_window_long(hwnd: HWND, nindex: WINDOW_LONG_PTR_INDEX, dwnewlong: isize) -> isize {
    #[cfg(target_pointer_width = "64")]
    unsafe {
        SetWindowLongPtrW(hwnd, nindex, dwnewlong)
    }
    #[cfg(target_pointer_width = "32")]
    unsafe {
        SetWindowLongW(hwnd, nindex, dwnewlong as i32) as isize
    }
}
