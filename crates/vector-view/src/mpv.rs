//! A small binding to libmpv: the client API (commands, properties, events) and the OpenGL
//! render API, which draws into the GLArea's framebuffer. Only what the viewer needs.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;

#[repr(C)]
struct Handle {
    _private: [u8; 0],
}

#[repr(C)]
pub struct RenderContext {
    _private: [u8; 0],
}

#[repr(C)]
struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct OpenGlInitParams {
    get_proc_address: extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    ctx: *mut c_void,
}

#[repr(C)]
struct OpenGlFbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

#[repr(C)]
struct RawEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct RawProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

type Callback = extern "C" fn(*mut c_void);

#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_create() -> *mut Handle;
    fn mpv_initialize(h: *mut Handle) -> c_int;
    fn mpv_terminate_destroy(h: *mut Handle);
    fn mpv_set_option_string(h: *mut Handle, name: *const c_char, data: *const c_char) -> c_int;
    fn mpv_command(h: *mut Handle, args: *mut *const c_char) -> c_int;
    fn mpv_set_property(
        h: *mut Handle,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    fn mpv_get_property(
        h: *mut Handle,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    fn mpv_observe_property(h: *mut Handle, id: u64, name: *const c_char, format: c_int) -> c_int;
    fn mpv_error_string(error: c_int) -> *const c_char;
    fn mpv_wait_event(h: *mut Handle, timeout: f64) -> *mut RawEvent;
    fn mpv_set_wakeup_callback(h: *mut Handle, cb: Callback, d: *mut c_void);
    fn mpv_render_context_create(
        res: *mut *mut RenderContext,
        h: *mut Handle,
        params: *mut RenderParam,
    ) -> c_int;
    fn mpv_render_context_set_update_callback(
        ctx: *mut RenderContext,
        cb: Callback,
        d: *mut c_void,
    );
    fn mpv_render_context_render(ctx: *mut RenderContext, params: *mut RenderParam) -> c_int;
    fn mpv_render_context_update(ctx: *mut RenderContext) -> u64;
    fn mpv_render_context_free(ctx: *mut RenderContext);
}

#[link(name = "EGL")]
unsafe extern "C" {
    fn eglGetProcAddress(name: *const c_char) -> *mut c_void;
}

const FORMAT_FLAG: c_int = 3;
const FORMAT_DOUBLE: c_int = 5;

const EVENT_SHUTDOWN: c_int = 1;
const EVENT_END_FILE: c_int = 7;
const EVENT_FILE_LOADED: c_int = 8;
const END_FILE_REASON_ERROR: c_int = 4;
const EVENT_PROPERTY_CHANGE: c_int = 22;

const RENDER_API_TYPE: c_int = 1;
const RENDER_OPENGL_INIT_PARAMS: c_int = 2;
const RENDER_OPENGL_FBO: c_int = 3;
const RENDER_FLIP_Y: c_int = 4;
const RENDER_WL_DISPLAY: c_int = 9;

const GL_FRAMEBUFFER_BINDING: u32 = 0x8CA6;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    None,
    Flag(bool),
    Double(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    FileLoaded,
    /// A file stopped; with mpv's error message when it failed.
    EndFile(Option<String>),
    Property(String, Value),
    Shutdown,
}

/// Wakes the main loop from mpv's threads. Bounded to one message: wakeups coalesce.
pub type Waker = async_channel::Sender<()>;

extern "C" fn wake(d: *mut c_void) {
    // SAFETY: `d` is the `Waker` boxed in `Mpv`, alive until mpv is destroyed.
    let tx = unsafe { &*(d as *const Waker) };
    let _ = tx.try_send(());
}

extern "C" fn proc_address(_: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: mpv passes a NUL-terminated function name.
    unsafe { eglGetProcAddress(name) }
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

pub struct Mpv {
    h: *mut Handle,
    _events: Box<Waker>,
}

impl Mpv {
    /// A player that draws through the render API. `options` are set before initialisation;
    /// `events` gets a message whenever `next_event` has something.
    pub fn new(options: &[(&str, &str)], events: Waker) -> Option<Mpv> {
        // SAFETY: plain libmpv calls on a handle we own; strings outlive each call.
        unsafe {
            let h = mpv_create();
            if h.is_null() {
                return None;
            }
            for (k, v) in options {
                mpv_set_option_string(h, cstr(k).as_ptr(), cstr(v).as_ptr());
            }
            if mpv_initialize(h) < 0 {
                mpv_terminate_destroy(h);
                return None;
            }
            let events = Box::new(events);
            mpv_set_wakeup_callback(h, wake, &*events as *const Waker as *mut c_void);
            Some(Mpv { h, _events: events })
        }
    }

    pub fn command(&self, args: &[&str]) -> bool {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());
        // SAFETY: NULL-terminated array of valid C strings.
        unsafe { mpv_command(self.h, ptrs.as_mut_ptr()) >= 0 }
    }

    pub fn set_f64(&self, name: &str, mut value: f64) {
        // SAFETY: MPV_FORMAT_DOUBLE takes a pointer to a double.
        unsafe {
            mpv_set_property(
                self.h,
                cstr(name).as_ptr(),
                FORMAT_DOUBLE,
                &mut value as *mut f64 as *mut c_void,
            );
        }
    }

    pub fn set_flag(&self, name: &str, value: bool) {
        let mut v: c_int = value.into();
        // SAFETY: MPV_FORMAT_FLAG takes a pointer to an int.
        unsafe {
            mpv_set_property(
                self.h,
                cstr(name).as_ptr(),
                FORMAT_FLAG,
                &mut v as *mut c_int as *mut c_void,
            );
        }
    }

    pub fn f64(&self, name: &str) -> Option<f64> {
        let mut v = 0f64;
        // SAFETY: as in `set_f64`.
        let r = unsafe {
            mpv_get_property(
                self.h,
                cstr(name).as_ptr(),
                FORMAT_DOUBLE,
                &mut v as *mut f64 as *mut c_void,
            )
        };
        (r >= 0).then_some(v)
    }

    pub fn observe_f64(&self, name: &str) {
        // SAFETY: plain call; the name is copied by mpv.
        unsafe { mpv_observe_property(self.h, 0, cstr(name).as_ptr(), FORMAT_DOUBLE) };
    }

    pub fn observe_flag(&self, name: &str) {
        // SAFETY: as above.
        unsafe { mpv_observe_property(self.h, 0, cstr(name).as_ptr(), FORMAT_FLAG) };
    }

    /// The next queued event, without waiting.
    pub fn next_event(&self) -> Option<Event> {
        loop {
            // SAFETY: the returned event stays valid until the next mpv_wait_event call.
            let e = unsafe { &*mpv_wait_event(self.h, 0.0) };
            return match e.event_id {
                0 => None,
                EVENT_SHUTDOWN => Some(Event::Shutdown),
                EVENT_FILE_LOADED => Some(Event::FileLoaded),
                EVENT_END_FILE => {
                    // mpv_event_end_file starts with (reason, error)
                    let failed = (!e.data.is_null())
                        .then(|| unsafe { *(e.data as *const [c_int; 2]) })
                        .filter(|[reason, _]| *reason == END_FILE_REASON_ERROR)
                        .map(|[_, error]| {
                            // SAFETY: mpv returns a static string for every error code.
                            unsafe { CStr::from_ptr(mpv_error_string(error)) }
                                .to_string_lossy()
                                .into_owned()
                        });
                    Some(Event::EndFile(failed))
                }
                EVENT_PROPERTY_CHANGE if !e.data.is_null() => {
                    // SAFETY: property-change events carry an mpv_event_property.
                    let p = unsafe { &*(e.data as *const RawProperty) };
                    let name = unsafe { CStr::from_ptr(p.name) }
                        .to_string_lossy()
                        .into_owned();
                    let value = match p.format {
                        FORMAT_DOUBLE if !p.data.is_null() => {
                            Value::Double(unsafe { *(p.data as *const f64) })
                        }
                        FORMAT_FLAG if !p.data.is_null() => {
                            Value::Flag(unsafe { *(p.data as *const c_int) } != 0)
                        }
                        _ => Value::None,
                    };
                    Some(Event::Property(name, value))
                }
                _ => continue,
            };
        }
    }

    /// The render context for the current OpenGL context (call with it made current).
    /// `update` gets a message whenever a new frame should be drawn.
    /// `wl_display` is the Wayland connection (or null), which some hardware decoders need.
    pub fn render_context(&self, update: Waker, wl_display: *mut c_void) -> Option<Render> {
        let api = cstr("opengl");
        let mut init = OpenGlInitParams {
            get_proc_address: proc_address,
            ctx: ptr::null_mut(),
        };
        let mut params = [
            RenderParam {
                kind: RENDER_API_TYPE,
                data: api.as_ptr() as *mut c_void,
            },
            RenderParam {
                kind: RENDER_OPENGL_INIT_PARAMS,
                data: &mut init as *mut OpenGlInitParams as *mut c_void,
            },
            RenderParam {
                kind: if wl_display.is_null() {
                    0
                } else {
                    RENDER_WL_DISPLAY
                },
                data: wl_display,
            },
            RenderParam {
                kind: 0,
                data: ptr::null_mut(),
            },
        ];
        let mut ctx: *mut RenderContext = ptr::null_mut();
        // SAFETY: params are valid for the call; the update waker is boxed in the result and
        // outlives the context (freed first in Drop).
        unsafe {
            if mpv_render_context_create(&mut ctx, self.h, params.as_mut_ptr()) < 0 || ctx.is_null()
            {
                return None;
            }
            let update = Box::new(update);
            mpv_render_context_set_update_callback(
                ctx,
                wake,
                &*update as *const Waker as *mut c_void,
            );
            let get = eglGetProcAddress(c"glGetIntegerv".as_ptr());
            Some(Render {
                ctx,
                _update: update,
                get_integer: (!get.is_null()).then(|| {
                    std::mem::transmute::<*mut c_void, extern "C" fn(u32, *mut c_int)>(get)
                }),
            })
        }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // SAFETY: the handle is ours; render contexts are dropped before the player.
        unsafe { mpv_terminate_destroy(self.h) };
    }
}

pub struct Render {
    ctx: *mut RenderContext,
    _update: Box<Waker>,
    get_integer: Option<extern "C" fn(u32, *mut c_int)>,
}

impl Render {
    /// Runs the work mpv queued for the render thread (screenshots wait on it) and tells
    /// whether a new frame is ready. Call with the GL context current, after each update.
    pub fn update(&self) -> bool {
        const UPDATE_FRAME: u64 = 1;
        // SAFETY: the context is alive.
        unsafe { mpv_render_context_update(self.ctx) & UPDATE_FRAME != 0 }
    }

    /// Draws the current frame into the bound framebuffer, `w`×`h` device pixels.
    pub fn draw(&self, w: i32, h: i32) {
        let mut fbo: c_int = 0;
        if let Some(get) = self.get_integer {
            get(GL_FRAMEBUFFER_BINDING, &mut fbo);
        }
        let mut target = OpenGlFbo {
            fbo,
            w,
            h,
            internal_format: 0,
        };
        let mut flip: c_int = 1;
        let mut params = [
            RenderParam {
                kind: RENDER_OPENGL_FBO,
                data: &mut target as *mut OpenGlFbo as *mut c_void,
            },
            RenderParam {
                kind: RENDER_FLIP_Y,
                data: &mut flip as *mut c_int as *mut c_void,
            },
            RenderParam {
                kind: 0,
                data: ptr::null_mut(),
            },
        ];
        // SAFETY: the context is alive; params are valid for the call.
        unsafe { mpv_render_context_render(self.ctx, params.as_mut_ptr()) };
    }
}

impl Drop for Render {
    fn drop(&mut self) {
        // SAFETY: freed once, with its GL context current (the caller makes it so).
        unsafe { mpv_render_context_free(self.ctx) };
    }
}
