//! A WebView2 browser inside one of the app's windows, for dashboards.
//!
//! The browser runs in its own processes; nothing starts until a view
//! creates one, and closing it lets them exit. It draws in a child window
//! over the part of the parent the view gives it, so the parent's Direct2D
//! frame never shows there: a view that draws over the page, such as a
//! dialog, hides the browser and draws a [`Snapshot`] of it instead.
//!
//! Every callback runs on the UI thread, from the message loop.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    AcceleratorKeyPressedEventHandler, CapturePreviewCompletedHandler, CoTaskMemPWSTR,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DocumentTitleChangedEventHandler, NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, ProcessFailedEventHandler, WebMessageReceivedEventHandler,
    WebResourceRequestedEventHandler,
};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, ID2D1Bitmap};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::IStream;
use windows::Win32::UI::WindowsAndMessaging::{GW_CHILD, GetWindow};
use windows::core::{HSTRING, Interface, PWSTR};

use super::geometry::Rect;
use super::gfx::Canvas;
use super::keys::KeyEvent;

/// What to do with a request the page makes.
pub enum Resource {
    /// Let it go to the network.
    Pass,
    /// Answer it with these bytes.
    Serve {
        content_type: String,
        body: Vec<u8>,
        /// More headers, such as a content security policy.
        headers: Vec<(String, String)>,
    },
    /// Answer 404.
    Missing,
    /// Answer 403, for a request the page may not make.
    Blocked,
}

/// How the page's load went, and the browser with it.
#[derive(Clone, Debug)]
pub enum WebViewState {
    /// The browser is up and the page is loading.
    Created,
    /// The page finished loading, or failed with this status.
    Loaded(Result<(), String>),
    /// The page's `<title>` changed.
    Title(String),
    /// The browser could not start, or one of its processes died.
    Failed(String),
}

pub struct WebViewCallbacks {
    /// A message the page sent with `chrome.webview.postMessage`, as JSON.
    pub message: Box<dyn Fn(String)>,
    /// A key pressed with Ctrl or Alt, or one that types nothing, while the
    /// page has the keyboard; true keeps it from the page.
    pub accelerator: Box<dyn Fn(&KeyEvent) -> bool>,
    /// A request for `url`, which the page is about to make.
    pub resource: Box<dyn Fn(&str) -> Resource>,
    /// Whether the page may navigate to `url`.
    pub navigation: Box<dyn Fn(&str) -> bool>,
    pub state: Box<dyn Fn(WebViewState)>,
}

pub struct WebViewOptions {
    /// Where the browser keeps its profile and cache.
    pub user_data: PathBuf,
    pub url: String,
    /// Runs in every page before the page's own scripts.
    pub script: String,
    /// The page's color before it paints, as 0xRRGGBB.
    pub background: u32,
}

#[derive(Default)]
struct Inner {
    environment: Option<ICoreWebView2Environment>,
    controller: Option<ICoreWebView2Controller>,
    webview: Option<ICoreWebView2>,
    bounds: RECT,
    visible: bool,
    /// Messages posted before the browser was up.
    queued: Vec<String>,
    closed: bool,
}

/// One browser. Dropping it closes the browser.
pub struct WebView {
    inner: Rc<RefCell<Inner>>,
    parent: HWND,
}

fn error_text(error: windows::core::Error) -> String {
    let message = error.message();
    if message.is_empty() { format!("{:#010x}", error.code().0) } else { format!("{message} ({:#010x})", error.code().0) }
}

impl WebView {
    /// Starts a browser in `parent`. It shows once the view gives it bounds
    /// and makes it visible; `callbacks.state` hears how starting went.
    pub fn create(parent: HWND, options: WebViewOptions, callbacks: WebViewCallbacks) -> Result<Self, String> {
        let inner = Rc::new(RefCell::new(Inner {
            visible: true,
            ..Inner::default()
        }));
        let callbacks = Rc::new(callbacks);
        let _ = std::fs::create_dir_all(&options.user_data);
        let user_data = HSTRING::from(options.user_data.as_os_str());
        let weak = Rc::downgrade(&inner);
        let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |result, environment| {
            let started = result.and_then(|()| environment.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER)));
            match started {
                Ok(environment) => {
                    if let Err(error) = create_controller(weak, parent, environment, options, callbacks.clone()) {
                        (callbacks.state)(WebViewState::Failed(error_text(error)));
                    }
                }
                Err(error) => (callbacks.state)(WebViewState::Failed(start_error(error))),
            }
            Ok(())
        }));
        unsafe { CreateCoreWebView2EnvironmentWithOptions(None, &user_data, None, &handler) }.map_err(start_error)?;
        Ok(Self { inner, parent })
    }

    /// Places the browser over `bounds`, in the parent's client pixels.
    pub fn set_bounds(&self, bounds: RECT) {
        let mut inner = self.inner.borrow_mut();
        if inner.bounds == bounds {
            return;
        }
        inner.bounds = bounds;
        if let Some(controller) = &inner.controller {
            let _ = unsafe { controller.SetBounds(bounds) };
        }
    }

    pub fn set_visible(&self, visible: bool) {
        let mut inner = self.inner.borrow_mut();
        if inner.visible == visible {
            return;
        }
        inner.visible = visible;
        if let Some(controller) = &inner.controller {
            let _ = unsafe { controller.SetIsVisible(visible) };
        }
    }

    pub fn is_ready(&self) -> bool {
        self.inner.borrow().webview.is_some()
    }

    /// Sends the page a message, which it gets as a `message` event's `data`.
    pub fn post_json(&self, json: &str) {
        let mut inner = self.inner.borrow_mut();
        match &inner.webview {
            Some(webview) => {
                let _ = unsafe { webview.PostWebMessageAsJson(&HSTRING::from(json)) };
            }
            None => inner.queued.push(json.to_owned()),
        }
    }

    pub fn reload(&self) {
        if let Some(webview) = &self.inner.borrow().webview {
            let _ = unsafe { webview.Reload() };
        }
    }

    pub fn open_dev_tools(&self) {
        if let Some(webview) = &self.inner.borrow().webview {
            let _ = unsafe { webview.OpenDevToolsWindow() };
        }
    }

    /// Gives the page the keyboard.
    pub fn focus(&self) {
        if let Some(controller) = &self.inner.borrow().controller {
            let _ = unsafe { controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC) };
        }
    }

    /// Tells the browser its window moved, so its popups, such as a
    /// dropdown's list, open in the right place.
    pub fn parent_moved(&self) {
        if let Some(controller) = &self.inner.borrow().controller {
            let _ = unsafe { controller.NotifyParentWindowPositionChanged() };
        }
    }

    /// Lets a hidden browser use less memory, or back to normal.
    pub fn set_low_memory(&self, low: bool) {
        if let Some(webview) = &self.inner.borrow().webview
            && let Ok(webview) = webview.cast::<ICoreWebView2_19>()
        {
            let level = if low { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW } else { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL };
            let _ = unsafe { webview.SetMemoryUsageTargetLevel(level) };
        }
    }

    /// Saves what the page shows as a PNG at `path`, then calls `done`.
    pub fn capture_png(&self, path: PathBuf, done: Box<dyn FnOnce(Result<(), String>)>) {
        let Some(webview) = self.inner.borrow().webview.clone() else {
            done(Err("the page is not open yet".into()));
            return;
        };
        let stream: Option<IStream> = unsafe { windows::Win32::UI::Shell::SHCreateMemStream(None) };
        let Some(stream) = stream else {
            done(Err("cannot make a memory stream".into()));
            return;
        };
        let written = stream.clone();
        let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
            done(result.map_err(error_text).and_then(|()| {
                let bytes = read_stream(&written)?;
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&path, bytes).map_err(|error| error.to_string())
            }));
            Ok(())
        }));
        let started = unsafe { webview.CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, &stream, &handler) };
        if let Err(error) = started {
            eprintln!("cannot capture the page: {}", error_text(error));
        }
    }

    /// What the browser shows now, to draw in its place while it is hidden.
    pub fn snapshot(&self) -> Option<Snapshot> {
        let (bounds, visible) = {
            let inner = self.inner.borrow();
            inner.controller.as_ref()?;
            (inner.bounds, inner.visible)
        };
        if !visible {
            return None;
        }
        let child = unsafe { GetWindow(self.parent, GW_CHILD) }.ok()?;
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        if width <= 0 || height <= 0 {
            return None;
        }
        unsafe {
            let screen = GetDC(None);
            let memory = CreateCompatibleDC(Some(screen));
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Negative: rows top to bottom.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok();
            let mut pixels = None;
            if let Some(bitmap) = bitmap {
                let old = SelectObject(memory, bitmap.into());
                // Draws the browser's own frame, even where another window covers it.
                let printed = windows::Win32::Storage::Xps::PrintWindow(child, memory, windows::Win32::Storage::Xps::PRINT_WINDOW_FLAGS(2));
                if printed.as_bool() && !bits.is_null() {
                    let length = (width * height * 4) as usize;
                    let mut copy = std::slice::from_raw_parts(bits as *const u8, length).to_vec();
                    // GDI leaves alpha at zero; the bitmap is opaque.
                    for alpha in copy.iter_mut().skip(3).step_by(4) {
                        *alpha = 255;
                    }
                    pixels = Some(copy);
                }
                SelectObject(memory, old);
                let _ = DeleteObject(bitmap.into());
            }
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
            Some(Snapshot {
                width: width as u32,
                height: height as u32,
                pixels: pixels?,
                bitmap: RefCell::new(None),
            })
        }
    }

    pub fn close(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.closed {
            return;
        }
        inner.closed = true;
        if let Some(controller) = inner.controller.take() {
            let _ = unsafe { controller.Close() };
        }
        inner.webview = None;
        inner.environment = None;
    }
}

impl Drop for WebView {
    fn drop(&mut self) {
        self.close();
    }
}

fn start_error(error: windows::core::Error) -> String {
    // HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND): no WebView2 Runtime installed.
    if error.code().0 as u32 == 0x8007_0002 {
        "The Microsoft Edge WebView2 Runtime is not installed. It comes with Windows 11; on Windows 10, \
         install it from https://developer.microsoft.com/microsoft-edge/webview2/"
            .into()
    } else {
        format!("Cannot start WebView2: {}", error_text(error))
    }
}

fn create_controller(
    weak: Weak<RefCell<Inner>>,
    parent: HWND,
    environment: ICoreWebView2Environment,
    options: WebViewOptions,
    callbacks: Rc<WebViewCallbacks>,
) -> windows::core::Result<()> {
    let Some(inner) = weak.upgrade() else {
        return Ok(());
    };
    if inner.borrow().closed {
        return Ok(());
    }
    inner.borrow_mut().environment = Some(environment.clone());
    let handler_environment = environment.clone();
    let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |result, controller| {
        let made = result.and_then(|()| controller.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER)));
        match made.and_then(|controller| set_up(&weak, handler_environment, controller, &options, callbacks.clone())) {
            Ok(()) => (callbacks.state)(WebViewState::Created),
            Err(error) => (callbacks.state)(WebViewState::Failed(format!("Cannot start WebView2: {}", error_text(error)))),
        }
        Ok(())
    }));
    unsafe { environment.CreateCoreWebView2Controller(parent, &handler) }
}

fn set_up(
    weak: &Weak<RefCell<Inner>>,
    environment: ICoreWebView2Environment,
    controller: ICoreWebView2Controller,
    options: &WebViewOptions,
    callbacks: Rc<WebViewCallbacks>,
) -> windows::core::Result<()> {
    let Some(inner) = weak.upgrade() else {
        unsafe { controller.Close()? };
        return Ok(());
    };
    if inner.borrow().closed {
        unsafe { controller.Close()? };
        return Ok(());
    }
    let webview = unsafe { controller.CoreWebView2()? };
    unsafe {
        if let Ok(controller) = controller.cast::<ICoreWebView2Controller2>() {
            let color = options.background;
            controller.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                A: 255,
                R: (color >> 16) as u8,
                G: (color >> 8) as u8,
                B: color as u8,
            })?;
        }
        let settings = webview.Settings()?;
        settings.SetAreDevToolsEnabled(true)?;
        settings.SetAreDefaultContextMenusEnabled(false)?;
        settings.SetIsStatusBarEnabled(false)?;
        settings.SetIsZoomControlEnabled(false)?;
        // Ctrl+P, F5, Ctrl+F and the rest belong to DataDiver, not the browser.
        if let Ok(settings) = settings.cast::<ICoreWebView2Settings3>() {
            settings.SetAreBrowserAcceleratorKeysEnabled(false)?;
        }
        // No "save password" or address prompts over a dashboard.
        if let Ok(settings) = settings.cast::<ICoreWebView2Settings4>() {
            settings.SetIsPasswordAutosaveEnabled(false)?;
            settings.SetIsGeneralAutofillEnabled(false)?;
        }

        let mut token = 0i64;
        let on_message = callbacks.clone();
        webview.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut json = PWSTR::null();
                    if args.WebMessageAsJson(&mut json).is_ok() {
                        let json = CoTaskMemPWSTR::from(json);
                        (on_message.message)(json.to_string());
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;

        let on_key = callbacks.clone();
        controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN {
                    return Ok(());
                }
                let mut vk = 0u32;
                args.VirtualKey(&mut vk)?;
                let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
                args.PhysicalKeyStatus(&mut status)?;
                let Some(key) = super::window::key_name(vk as u16, status.IsExtendedKey.as_bool()) else {
                    return Ok(());
                };
                let modifiers = super::window::modifiers();
                let typed = if modifiers.control || modifiers.alt {
                    None
                } else {
                    super::window::typed_text(vk as u16, status.ScanCode)
                };
                let event = KeyEvent::new(key, modifiers, typed);
                if (on_key.accelerator)(&event) {
                    args.SetHandled(true)?;
                }
                Ok(())
            })),
            &mut token,
        )?;

        webview.AddWebResourceRequestedFilter(&HSTRING::from("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        let on_resource = callbacks.clone();
        let responses = environment.clone();
        webview.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let request = args.Request()?;
                let mut uri = PWSTR::null();
                request.Uri(&mut uri)?;
                let uri = CoTaskMemPWSTR::from(uri).to_string();
                let (status, reason, content_type, body, headers) = match (on_resource.resource)(&uri) {
                    Resource::Pass => return Ok(()),
                    Resource::Serve { content_type, body, headers } => (200, "OK", content_type, body, headers),
                    Resource::Missing => (404, "Not Found", "text/plain".into(), b"Not found".to_vec(), Vec::new()),
                    Resource::Blocked => (
                        403,
                        "Forbidden",
                        "text/plain".into(),
                        b"DataDiver dashboards may not reach this address".to_vec(),
                        Vec::new(),
                    ),
                };
                let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(&body));
                let mut header_text = format!("Content-Type: {content_type}");
                for (name, value) in headers {
                    header_text.push_str(&format!("\r\n{name}: {value}"));
                }
                let response = responses.CreateWebResourceResponse(
                    stream.as_ref(),
                    status,
                    &HSTRING::from(reason),
                    &HSTRING::from(header_text),
                )?;
                args.SetResponse(&response)?;
                Ok(())
            })),
            &mut token,
        )?;

        let on_navigation = callbacks.clone();
        webview.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    let uri = CoTaskMemPWSTR::from(uri).to_string();
                    if !(on_navigation.navigation)(&uri) {
                        args.SetCancel(true)?;
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;

        // Links that would open a window go nowhere.
        webview.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    args.SetHandled(true)?;
                }
                Ok(())
            })),
            &mut token,
        )?;

        let on_loaded = callbacks.clone();
        webview.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut success = windows::core::BOOL::default();
                    args.IsSuccess(&mut success)?;
                    let outcome = if success.as_bool() {
                        Ok(())
                    } else {
                        let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                        args.WebErrorStatus(&mut status)?;
                        Err(format!("the page did not load (WebView2 error {})", status.0))
                    };
                    (on_loaded.state)(WebViewState::Loaded(outcome));
                }
                Ok(())
            })),
            &mut token,
        )?;

        let on_title = callbacks.clone();
        webview.add_DocumentTitleChanged(
            &DocumentTitleChangedEventHandler::create(Box::new(move |sender, _| {
                if let Some(sender) = sender {
                    let mut title = PWSTR::null();
                    sender.DocumentTitle(&mut title)?;
                    (on_title.state)(WebViewState::Title(CoTaskMemPWSTR::from(title).to_string()));
                }
                Ok(())
            })),
            &mut token,
        )?;

        let on_failure = callbacks.clone();
        webview.add_ProcessFailed(
            &ProcessFailedEventHandler::create(Box::new(move |_, args| {
                let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                if let Some(args) = args {
                    args.ProcessFailedKind(&mut kind)?;
                }
                (on_failure.state)(WebViewState::Failed(format!(
                    "The dashboard's browser process stopped (kind {}); reload to start it again",
                    kind.0
                )));
                Ok(())
            })),
            &mut token,
        )?;

        webview.AddScriptToExecuteOnDocumentCreated(&HSTRING::from(options.script.as_str()), None)?;
    }
    let (bounds, visible, queued) = {
        let mut state = inner.borrow_mut();
        state.controller = Some(controller.clone());
        state.webview = Some(webview.clone());
        (state.bounds, state.visible, std::mem::take(&mut state.queued))
    };
    unsafe {
        controller.SetBounds(bounds)?;
        controller.SetIsVisible(visible)?;
        webview.Navigate(&HSTRING::from(options.url.as_str()))?;
        for json in queued {
            webview.PostWebMessageAsJson(&HSTRING::from(json))?;
        }
    }
    Ok(())
}

fn read_stream(stream: &IStream) -> Result<Vec<u8>, String> {
    use windows::Win32::System::Com::{STATSTG, STATFLAG_NONAME, STREAM_SEEK_SET};
    unsafe {
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME).map_err(error_text)?;
        stream.Seek(0, STREAM_SEEK_SET, None).map_err(error_text)?;
        let mut bytes = vec![0u8; stat.cbSize as usize];
        let mut read = 0u32;
        stream
            .Read(bytes.as_mut_ptr() as *mut _, bytes.len() as u32, Some(&mut read))
            .ok()
            .map_err(error_text)?;
        bytes.truncate(read as usize);
        Ok(bytes)
    }
}

/// A picture of a browser's page, drawn where the browser was.
pub struct Snapshot {
    width: u32,
    height: u32,
    /// Premultiplied BGRA, opaque, rows top to bottom.
    pixels: Vec<u8>,
    /// The bitmap made from `pixels` on the render target it was drawn on.
    bitmap: RefCell<Option<(usize, ID2D1Bitmap)>>,
}

impl Snapshot {
    pub fn draw<H: Clone>(&self, canvas: &mut Canvas<H>, r: Rect) {
        canvas.paint_native(|target| unsafe {
            let key = target.as_raw() as usize;
            let mut cached = self.bitmap.borrow_mut();
            if cached.as_ref().is_none_or(|(owner, _)| *owner != key) {
                let properties = D2D1_BITMAP_PROPERTIES {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                };
                let made = target.CreateBitmap(
                    D2D_SIZE_U {
                        width: self.width,
                        height: self.height,
                    },
                    Some(self.pixels.as_ptr() as *const _),
                    self.width * 4,
                    &properties,
                );
                *cached = made.ok().map(|bitmap| (key, bitmap));
            }
            if let Some((_, bitmap)) = cached.as_ref() {
                let destination = D2D_RECT_F {
                    left: r.x,
                    top: r.y,
                    right: r.x + r.w,
                    bottom: r.y + r.h,
                };
                target.DrawBitmap(bitmap, Some(&destination), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            }
        });
    }
}
