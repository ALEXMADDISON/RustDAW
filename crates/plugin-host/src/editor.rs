//! Native plugin editor windows (VST3 `IPlugView`).
//!
//! Windows: the host creates a top-level window, hands its HWND to the plugin (`attached`) and
//! resizes it when the plugin asks (`IPlugFrame::resizeView`). Everything runs on the GUI thread;
//! eframe/winit's message loop dispatches this window's messages as well, so plugin timers and
//! mouse input just work.
//! Other platforms: not hosted yet (`PluginController::create_editor` returns an error and the
//! GUI falls back to a generic parameter list).

#[cfg(windows)]
pub use win::PluginEditor;

#[cfg(not(windows))]
pub use stub::PluginEditor;

#[cfg(not(windows))]
mod stub {
    /// Placeholder on platforms where plugin windows are not hosted yet.
    pub struct PluginEditor;

    impl PluginEditor {
        pub fn is_closed(&self) -> bool {
            true
        }
        pub fn bring_to_front(&self) {}
    }
}

#[cfg(windows)]
mod win {
    use crate::module::Module;
    use std::cell::Cell;
    use std::ffi::{c_char, c_void};
    use std::sync::{Arc, Once};
    use vst3::{Class, ComPtr, ComRef, ComWrapper, Steinberg::Vst::*, Steinberg::*};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::COLOR_BTNFACE;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const HWND_TYPE: *const c_char = b"HWND\0".as_ptr() as *const c_char;
    const CLASS_NAME: &str = "RustDawPluginEditor";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// State shared between the window procedure, the frame object and the editor.
    struct WinState {
        closed: Cell<bool>,
        busy: Cell<bool>,
        can_resize: bool,
        view: Cell<*mut IPlugView>,
    }

    // ---- IPlugFrame: the plugin asks the host to resize its window ----
    struct Frame {
        hwnd: Cell<HWND>,
        state: Cell<*const WinState>,
    }
    impl Class for Frame {
        type Interfaces = (IPlugFrame,);
    }
    impl IPlugFrameTrait for Frame {
        unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
            if new_size.is_null() {
                return kInvalidArgument;
            }
            let mut r = *new_size;
            set_client_size(self.hwnd.get(), r.right - r.left, r.bottom - r.top);
            // Tell the plugin the size it now has (spec: host answers with onSize).
            let st = self.state.get();
            if !st.is_null() {
                (*st).busy.set(true);
            }
            if let Some(v) = ComRef::from_raw(view) {
                let mut cr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                GetClientRect(self.hwnd.get(), &mut cr);
                r.left = 0;
                r.top = 0;
                r.right = cr.right;
                r.bottom = cr.bottom;
                let _ = v.onSize(&mut r);
            }
            if !st.is_null() {
                (*st).busy.set(false);
            }
            kResultTrue
        }
    }

    fn set_client_size(hwnd: HWND, w: i32, h: i32) {
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let mut rc = RECT { left: 0, top: 0, right: w.max(1), bottom: h.max(1) };
            AdjustWindowRectEx(&mut rc, style, 0, 0);
            SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, rc.right - rc.left, rc.bottom - rc.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let st = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WinState;
        if !st.is_null() {
            let st = &*st;
            match msg {
                WM_CLOSE => {
                    // The GUI drops the editor on its next frame; just hide it now.
                    st.closed.set(true);
                    ShowWindow(hwnd, SW_HIDE);
                    return 0;
                }
                WM_SIZE => {
                    if st.can_resize && !st.busy.get() && wp as u32 != SIZE_MINIMIZED {
                        if let Some(v) = ComRef::from_raw(st.view.get()) {
                            st.busy.set(true);
                            let mut cr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                            GetClientRect(hwnd, &mut cr);
                            let mut r = ViewRect { left: 0, top: 0, right: cr.right, bottom: cr.bottom };
                            let _ = v.checkSizeConstraint(&mut r);
                            let _ = v.onSize(&mut r);
                            st.busy.set(false);
                        }
                    }
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }

    fn register_class() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe {
            let name = wide(CLASS_NAME);
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(std::ptr::null()),
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: (COLOR_BTNFACE + 1) as usize as _,
                lpszMenuName: std::ptr::null(),
                lpszClassName: name.as_ptr(),
            };
            RegisterClassW(&wc);
        });
    }

    /// An open plugin editor window. Must be created, used and dropped on the GUI thread.
    pub struct PluginEditor {
        hwnd: HWND,
        view: ComPtr<IPlugView>,
        frame: ComWrapper<Frame>,
        state: Box<WinState>,
        _module: Arc<Module>,
    }

    impl PluginEditor {
        pub(crate) fn open(controller: &ComPtr<IEditController>, module: Arc<Module>, title: &str) -> Result<PluginEditor, String> {
            unsafe {
                let raw = controller.createView(b"editor\0".as_ptr() as *const c_char);
                let view = ComPtr::from_raw(raw).ok_or("This plugin has no editor window")?;
                if view.isPlatformTypeSupported(HWND_TYPE) != kResultTrue {
                    return Err("The plugin's editor does not support Windows (HWND)".into());
                }
                let mut size = ViewRect { left: 0, top: 0, right: 0, bottom: 0 };
                if view.getSize(&mut size) != kResultOk || size.right - size.left < 10 || size.bottom - size.top < 10 {
                    size = ViewRect { left: 0, top: 0, right: 640, bottom: 480 };
                }
                let can_resize = view.canResize() == kResultTrue;
                let style = if can_resize { WS_OVERLAPPEDWINDOW } else { WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX };

                register_class();
                let mut rc = RECT { left: 0, top: 0, right: size.right - size.left, bottom: size.bottom - size.top };
                AdjustWindowRectEx(&mut rc, style, 0, 0);
                let class = wide(CLASS_NAME);
                let title_w = wide(title);
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title_w.as_ptr(),
                    style,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    rc.right - rc.left,
                    rc.bottom - rc.top,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    GetModuleHandleW(std::ptr::null()),
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return Err("could not create the editor window".into());
                }

                let state = Box::new(WinState { closed: Cell::new(false), busy: Cell::new(true), can_resize, view: Cell::new(view.as_ptr()) });
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, &*state as *const WinState as isize);
                let frame = ComWrapper::new(Frame { hwnd: Cell::new(hwnd), state: Cell::new(&*state as *const WinState) });
                let frame_ptr = match frame.to_com_ptr::<IPlugFrame>() {
                    Some(p) => p,
                    None => {
                        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                        DestroyWindow(hwnd);
                        return Err("frame interface".into());
                    }
                };
                let _ = view.setFrame(frame_ptr.as_ptr());
                if view.attached(hwnd as *mut c_void, HWND_TYPE) != kResultOk {
                    let _ = view.setFrame(std::ptr::null_mut());
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    DestroyWindow(hwnd);
                    return Err("the plugin refused to attach its editor".into());
                }
                state.busy.set(false);
                ShowWindow(hwnd, SW_SHOW);
                SetForegroundWindow(hwnd);
                Ok(PluginEditor { hwnd, view, frame, state, _module: module })
            }
        }

        /// True once the user closed the window (the GUI should then drop the editor).
        pub fn is_closed(&self) -> bool {
            self.state.closed.get()
        }

        pub fn bring_to_front(&self) {
            unsafe {
                ShowWindow(self.hwnd, SW_RESTORE);
                SetForegroundWindow(self.hwnd);
            }
        }
    }

    impl Drop for PluginEditor {
        fn drop(&mut self) {
            unsafe {
                self.state.busy.set(true);
                let _ = self.view.removed();
                let _ = self.view.setFrame(std::ptr::null_mut());
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                DestroyWindow(self.hwnd);
                let _ = &self.frame;
            }
        }
    }
}
