//! The application runtime: windows, background work, and timers.
//!
//! Everything a view owns lives on the UI thread. Work that may block runs on
//! a thread pool, and its result comes back to the UI thread, where a
//! continuation updates the view that started it. Timers run continuations at
//! a deadline. Both are cancelled when their [`Task`] handle drops, unless the
//! handle was detached.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use super::gfx::{Canvas, HitRegion};
use super::geometry::Point;
use super::keys::{KeyEvent, Modifiers};

pub type WindowId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Input for a view. Mouse events carry the hit region under the pointer.
#[derive(Clone, Debug)]
pub enum Event<H> {
    Key(KeyEvent),
    /// Typed text, after a key press the view did not handle.
    Text(String),
    MouseDown {
        pos: Point,
        button: MouseButton,
        modifiers: Modifiers,
        clicks: u32,
        hit: Option<H>,
    },
    MouseUp {
        pos: Point,
        button: MouseButton,
        modifiers: Modifiers,
        hit: Option<H>,
    },
    MouseMove {
        pos: Point,
        modifiers: Modifiers,
        left_down: bool,
        hit: Option<H>,
    },
    /// Wheel movement in lines; positive `y` scrolls toward the top.
    Wheel {
        pos: Point,
        delta: Point,
        modifiers: Modifiers,
        hit: Option<H>,
    },
    MouseLeave,
    Drop {
        paths: Vec<PathBuf>,
        pos: Point,
        hit: Option<H>,
    },
    Activated(bool),
    /// The window moved or changed size.
    Moved,
}

pub trait View: Sized + 'static {
    type Hit: Clone + PartialEq + std::fmt::Debug + 'static;

    fn paint(&mut self, canvas: &mut Canvas<Self::Hit>, cx: &mut Cx<Self>);

    /// Returns whether the view used the event. An unused key press lets its
    /// typed text through as [`Event::Text`].
    fn event(&mut self, event: &Event<Self::Hit>, cx: &mut Cx<Self>) -> bool;

    /// The window's close button or Alt+F4; false keeps it open.
    fn close_requested(&mut self, _cx: &mut Cx<Self>) -> bool {
        true
    }

    /// The window is going away, or the app is quitting.
    fn closing(&mut self, _cx: &mut Cx<Self>) {}

    /// Where the text cursor is in `window`, so an input method can place
    /// its window.
    fn caret(&self, _window: WindowId) -> Option<super::geometry::Rect> {
        None
    }
}

/// A pending continuation or timer. Dropping it cancels the work's result;
/// [`Task::detach`] lets it finish on its own.
#[must_use = "dropping a Task cancels it; call detach() to let it run"]
pub struct Task {
    alive: Rc<Cell<bool>>,
    detached: bool,
}

impl Task {
    pub fn detach(mut self) {
        self.detached = true;
    }

    pub fn is_pending(&self) -> bool {
        self.alive.get()
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if !self.detached {
            self.alive.set(false);
        }
    }
}

type Continuation<V> = Box<dyn FnOnce(&mut V, &mut Cx<V>, Box<dyn Any + Send>)>;
type TimerCallback<V> = Box<dyn FnOnce(&mut V, &mut Cx<V>)>;
type WindowBuilder<V> = Box<dyn FnOnce(&mut Cx<V>) -> V>;

struct PendingContinuation<V: View> {
    window: WindowId,
    alive: Rc<Cell<bool>>,
    run: Continuation<V>,
}

struct Timer<V: View> {
    deadline: Instant,
    window: WindowId,
    alive: Rc<Cell<bool>>,
    run: TimerCallback<V>,
}

pub(crate) struct OpenRequest<V: View> {
    pub options: WindowOptions,
    pub contents: Contents<V>,
    pub id: WindowId,
}

/// What a new window shows.
pub(crate) enum Contents<V: View> {
    /// A view of its own, which `build` makes.
    View(WindowBuilder<V>),
    /// The view of window `owner`, in a window kept above that one and closed
    /// with it. `opened` runs once the window exists.
    Owned { owner: WindowId, opened: TimerCallback<V> },
}

pub(crate) enum Action<V: View> {
    Open(OpenRequest<V>),
    Close(WindowId),
    Quit,
    Update(WindowId, TimerCallback<V>),
    /// Runs with no view borrowed, such as to paint a window.
    Runtime(Box<dyn FnOnce(&Runtime<V>)>),
}

/// Where and how big a new window is.
#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub title: String,
    /// Client size in DIPs, used without `placement`.
    pub width: f32,
    pub height: f32,
    /// A saved position in screen pixels.
    pub placement: Option<Placement>,
    pub visible: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "DataDiver".into(),
            width: 1400.0,
            height: 900.0,
            placement: None,
            visible: true,
        }
    }
}

/// A window's outer rectangle in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

type Job = Box<dyn FnOnce() + Send>;

struct Pool {
    sender: Mutex<mpsc::Sender<Job>>,
}

impl Pool {
    fn new() -> Self {
        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        let threads = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(4)
            .clamp(4, 16);
        for index in 0..threads {
            let receiver = receiver.clone();
            std::thread::Builder::new()
                .name(format!("datadiver-worker-{index}"))
                .spawn(move || {
                    loop {
                        let job = {
                            let Ok(receiver) = receiver.lock() else {
                                return;
                            };
                            receiver.recv()
                        };
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                })
                .expect("start a worker thread");
        }
        Self {
            sender: Mutex::new(sender),
        }
    }

    fn run(&self, job: Job) {
        if let Ok(sender) = self.sender.lock() {
            let _ = sender.send(job);
        }
    }
}

/// Wakes the UI thread when background work finishes.
pub(crate) type Waker = Arc<dyn Fn() + Send + Sync>;

pub(crate) struct Shared<V: View> {
    next_task: u64,
    pub(crate) next_window: WindowId,
    continuations: Vec<(u64, PendingContinuation<V>)>,
    timers: Vec<Timer<V>>,
    pub(crate) actions: VecDeque<Action<V>>,
    /// Continuations whose window was busy, retried on the next drain.
    deferred: Vec<(u64, Box<dyn Any + Send>)>,
}

/// State shared by every window of the app.
pub struct Runtime<V: View> {
    pub(crate) shared: RefCell<Shared<V>>,
    results_tx: mpsc::Sender<(u64, Box<dyn Any + Send>)>,
    results_rx: mpsc::Receiver<(u64, Box<dyn Any + Send>)>,
    pool: Pool,
    in_flight: Arc<AtomicUsize>,
    waker: RefCell<Option<Waker>>,
    clock_offset: Cell<Duration>,
    pub(crate) windows: RefCell<Vec<Rc<super::window::WindowSlot<V>>>>,
    pub(crate) quitting: Cell<bool>,
}

impl<V: View> Runtime<V> {
    pub(crate) fn new() -> Self {
        let (results_tx, results_rx) = mpsc::channel();
        Self {
            shared: RefCell::new(Shared {
                next_task: 1,
                next_window: 1,
                continuations: Vec::new(),
                timers: Vec::new(),
                actions: VecDeque::new(),
                deferred: Vec::new(),
            }),
            results_tx,
            results_rx,
            pool: Pool::new(),
            in_flight: Arc::new(AtomicUsize::new(0)),
            waker: RefCell::new(None),
            clock_offset: Cell::new(Duration::ZERO),
            windows: RefCell::new(Vec::new()),
            quitting: Cell::new(false),
        }
    }

    pub(crate) fn set_waker(&self, waker: Waker) {
        *self.waker.borrow_mut() = Some(waker);
    }

    pub fn now(&self) -> Instant {
        Instant::now() + self.clock_offset.get()
    }

    /// Moves the timer clock forward, for tests.
    pub fn advance_clock(&self, by: Duration) {
        self.clock_offset.set(self.clock_offset.get() + by);
    }

    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    pub(crate) fn window(&self, id: WindowId) -> Option<Rc<super::window::WindowSlot<V>>> {
        self.windows
            .borrow()
            .iter()
            .find(|slot| slot.id == id)
            .cloned()
    }

    /// Opens a window whose view `build` makes, once the event loop runs.
    pub fn open_window(
        &self,
        options: WindowOptions,
        build: impl FnOnce(&mut Cx<V>) -> V + 'static,
    ) -> WindowId {
        let id = self.next_window_id();
        self.queue(Action::Open(OpenRequest {
            options,
            contents: Contents::View(Box::new(build)),
            id,
        }));
        id
    }

    /// Closes every window, ending the app.
    pub fn quit(&self) {
        self.queue(Action::Quit);
    }

    pub(crate) fn queue(&self, action: Action<V>) {
        self.shared.borrow_mut().actions.push_back(action);
    }

    pub(crate) fn next_window_id(&self) -> WindowId {
        let mut shared = self.shared.borrow_mut();
        let id = shared.next_window;
        shared.next_window += 1;
        id
    }

    fn spawn_raw<T: Send + 'static>(
        &self,
        window: WindowId,
        work: impl FnOnce() -> T + Send + 'static,
        run: Continuation<V>,
    ) -> Task {
        let alive = Rc::new(Cell::new(true));
        let id = {
            let mut shared = self.shared.borrow_mut();
            let id = shared.next_task;
            shared.next_task += 1;
            shared.continuations.push((
                id,
                PendingContinuation {
                    window,
                    alive: alive.clone(),
                    run,
                },
            ));
            id
        };
        let sender = self.results_tx.clone();
        let waker = self.waker.borrow().clone();
        let in_flight = self.in_flight.clone();
        in_flight.fetch_add(1, Ordering::AcqRel);
        self.pool.run(Box::new(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            if let Ok(value) = result {
                let _ = sender.send((id, Box::new(value)));
            }
            in_flight.fetch_sub(1, Ordering::AcqRel);
            if let Some(waker) = waker {
                waker();
            }
        }));
        Task {
            alive,
            detached: false,
        }
    }

    fn timer_raw(&self, window: WindowId, delay: Duration, run: TimerCallback<V>) -> Task {
        let alive = Rc::new(Cell::new(true));
        self.shared.borrow_mut().timers.push(Timer {
            deadline: self.now() + delay,
            window,
            alive: alive.clone(),
            run,
        });
        Task {
            alive,
            detached: false,
        }
    }

    /// The next timer deadline, if any timer is waiting.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let shared = self.shared.borrow();
        shared
            .timers
            .iter()
            .filter(|timer| timer.alive.get())
            .map(|timer| timer.deadline)
            .min()
            .map(|deadline| deadline - self.clock_offset.get())
    }

    /// Runs finished work, due timers, and queued actions. Returns whether
    /// anything ran.
    pub(crate) fn drain(&self, open: &dyn Fn(&Self, OpenRequest<V>), close: &dyn Fn(&Self, WindowId)) -> bool {
        let mut ran = false;
        let mut results: Vec<(u64, Box<dyn Any + Send>)> =
            std::mem::take(&mut self.shared.borrow_mut().deferred);
        while let Ok(result) = self.results_rx.try_recv() {
            results.push(result);
        }
        for (id, value) in results {
            let pending = {
                let mut shared = self.shared.borrow_mut();
                shared
                    .continuations
                    .iter()
                    .position(|(task, _)| *task == id)
                    .map(|index| shared.continuations.remove(index).1)
            };
            let Some(pending) = pending else {
                continue;
            };
            if !pending.alive.get() {
                continue;
            }
            let Some(slot) = self.window(pending.window) else {
                continue;
            };
            let Ok(mut view) = slot.view.try_borrow_mut() else {
                // The view is busy, such as behind a modal file dialog.
                let mut shared = self.shared.borrow_mut();
                shared.continuations.push((id, pending));
                shared.deferred.push((id, value));
                continue;
            };
            ran = true;
            pending.alive.set(false);
            let mut cx = Cx {
                runtime: self,
                window: slot.id,
            };
            (pending.run)(&mut view, &mut cx, value);
        }
        loop {
            let now = self.now();
            let due = {
                let mut shared = self.shared.borrow_mut();
                shared.timers.retain(|timer| timer.alive.get());
                shared
                    .timers
                    .iter()
                    .enumerate()
                    .filter(|(_, timer)| timer.deadline <= now)
                    .min_by_key(|(_, timer)| timer.deadline)
                    .map(|(index, _)| index)
                    .map(|index| shared.timers.remove(index))
            };
            let Some(timer) = due else {
                break;
            };
            let Some(slot) = self.window(timer.window) else {
                continue;
            };
            let Ok(mut view) = slot.view.try_borrow_mut() else {
                self.shared.borrow_mut().timers.push(timer);
                break;
            };
            ran = true;
            timer.alive.set(false);
            let mut cx = Cx {
                runtime: self,
                window: slot.id,
            };
            (timer.run)(&mut view, &mut cx);
        }
        loop {
            let action = self.shared.borrow_mut().actions.pop_front();
            let Some(action) = action else {
                break;
            };
            ran = true;
            match action {
                Action::Open(request) => open(self, request),
                Action::Close(id) => close(self, id),
                Action::Quit => {
                    self.quitting.set(true);
                    let ids = self.windows.borrow().iter().map(|slot| slot.id).collect::<Vec<_>>();
                    for id in ids {
                        close(self, id);
                    }
                }
                Action::Runtime(run) => run(self),
                Action::Update(id, run) => {
                    let Some(slot) = self.window(id) else {
                        continue;
                    };
                    let Ok(mut view) = slot.view.try_borrow_mut() else {
                        self.shared.borrow_mut().actions.push_front(Action::Update(id, run));
                        break;
                    };
                    let mut cx = Cx {
                        runtime: self,
                        window: id,
                    };
                    run(&mut view, &mut cx);
                }
            }
        }
        ran
    }
}

/// What a view can do while it handles input or a continuation.
pub struct Cx<'a, V: View> {
    pub(crate) runtime: &'a Runtime<V>,
    pub(crate) window: WindowId,
}

impl<'a, V: View> Cx<'a, V> {
    pub fn window_id(&self) -> WindowId {
        self.window
    }

    pub fn runtime(&self) -> &'a Runtime<V> {
        self.runtime
    }

    /// Redraws the window, and any other window showing its view.
    pub fn notify(&mut self) {
        let Some(slot) = self.runtime.window(self.window) else {
            return;
        };
        let slots = self.runtime.windows.borrow().clone();
        for other in slots {
            if Rc::ptr_eq(&other.view, &slot.view) {
                other.invalidate();
            }
        }
    }

    /// Redraws every window, such as after a shared setting changed.
    pub fn notify_all(&mut self) {
        let slots = self.runtime.windows.borrow().clone();
        for slot in slots {
            slot.invalidate();
        }
    }

    /// Runs `work` on a worker thread, then `then` with its result on the UI
    /// thread, while this window is open and the task is alive.
    pub fn spawn<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> T + Send + 'static,
        then: impl FnOnce(&mut V, &mut Cx<V>, T) + 'static,
    ) -> Task {
        self.runtime.spawn_raw(
            self.window,
            work,
            Box::new(move |view, cx, value| {
                if let Ok(value) = value.downcast::<T>() {
                    then(view, cx, *value);
                }
            }),
        )
    }

    /// Runs `then` after `delay`.
    pub fn timer(&mut self, delay: Duration, then: impl FnOnce(&mut V, &mut Cx<V>) + 'static) -> Task {
        self.runtime.timer_raw(self.window, delay, Box::new(then))
    }

    /// Runs `then` with this window's view once the current handler returns.
    pub fn defer(&mut self, then: impl FnOnce(&mut V, &mut Cx<V>) + 'static) {
        self.runtime
            .queue(Action::Update(self.window, Box::new(then)));
    }

    /// Runs `then` with another window's view once the current handler returns.
    pub fn update_window(&mut self, window: WindowId, then: impl FnOnce(&mut V, &mut Cx<V>) + 'static) {
        self.runtime.queue(Action::Update(window, Box::new(then)));
    }

    /// Opens a window whose view `build` makes.
    pub fn open_window(
        &mut self,
        options: WindowOptions,
        build: impl FnOnce(&mut Cx<V>) -> V + 'static,
    ) -> WindowId {
        let id = self.runtime.next_window_id();
        self.runtime.queue(Action::Open(OpenRequest {
            options,
            contents: Contents::View(Box::new(build)),
            id,
        }));
        id
    }

    /// Opens another window onto this window's view, kept above this one and
    /// closed with it. The view tells the windows apart by [`Cx::window_id`].
    /// `opened` runs once the window exists.
    pub fn open_owned_window(
        &mut self,
        options: WindowOptions,
        opened: impl FnOnce(&mut V, &mut Cx<V>) + 'static,
    ) -> WindowId {
        let id = self.runtime.next_window_id();
        self.runtime.queue(Action::Open(OpenRequest {
            options,
            contents: Contents::Owned {
                owner: self.window,
                opened: Box::new(opened),
            },
            id,
        }));
        id
    }

    /// This context as window `window`'s, such as the window an owned window
    /// shows the view of. Work spawned through it belongs to that window.
    pub fn for_window(&self, window: WindowId) -> Cx<'a, V> {
        Cx {
            runtime: self.runtime,
            window,
        }
    }

    /// Runs `run` once no view is busy, with the whole runtime.
    pub fn with_runtime(&mut self, run: impl FnOnce(&Runtime<V>) + 'static) {
        self.runtime.queue(Action::Runtime(Box::new(run)));
    }

    pub fn close_window(&mut self) {
        self.runtime.queue(Action::Close(self.window));
    }

    pub fn quit(&mut self) {
        self.runtime.queue(Action::Quit);
    }

    pub fn window_count(&self) -> usize {
        self.runtime.windows.borrow().len()
    }

    /// The window's client size in DIPs.
    pub fn viewport(&self) -> (f32, f32) {
        self.runtime
            .window(self.window)
            .map(|slot| slot.viewport())
            .unwrap_or((1400.0, 900.0))
    }

    /// The window's outer rectangle in screen pixels.
    pub fn placement(&self) -> Option<Placement> {
        self.runtime
            .window(self.window)
            .and_then(|slot| slot.placement())
    }

    /// The window's outer rectangle in screen pixels as it is now.
    pub fn window_rect(&self) -> Option<super::geometry::Rect> {
        self.runtime
            .window(self.window)
            .and_then(|slot| slot.outer_rect())
    }

    pub fn activate(&mut self) {
        if let Some(slot) = self.runtime.window(self.window) {
            slot.activate();
        }
    }

    /// The hit region under `pos` in the last frame.
    pub fn hit_at(&self, pos: Point) -> Option<V::Hit> {
        self.runtime
            .window(self.window)
            .and_then(|slot| slot.hit_at(pos))
    }

    /// The last frame's hit regions.
    pub fn hits(&self) -> Vec<HitRegion<V::Hit>> {
        self.runtime
            .window(self.window)
            .map(|slot| slot.state.borrow().hits.clone())
            .unwrap_or_default()
    }

    pub fn now(&self) -> Instant {
        self.runtime.now()
    }

    pub(crate) fn hwnd(&self) -> Option<windows::Win32::Foundation::HWND> {
        self.runtime
            .window(self.window)
            .and_then(|slot| slot.hwnd())
    }
}
