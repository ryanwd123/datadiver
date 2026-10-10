use datadiver::ui::input_with_autocompletion::InputWithAutocompletion;
use gpui_kit::component::Root;
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Window, WindowBounds, WindowOptions, div, point, px, size,
};

struct CompletionDemo {
    input: Entity<InputWithAutocompletion>,
}

impl CompletionDemo {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputWithAutocompletion::new(["apple", "apricot", "banana", "potato"], window, cx)
        });
        Self { input }
    }
}

impl Render for CompletionDemo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .w_full()
            .child(div().text_xl().child("SQL input completion demo"))
            .child(
                div()
                    .text_sm()
                    .child("Type a column name, such as ap, in a SQL expression."),
            )
            .child(self.input.clone())
            .child(
                div().text_sm().child(
                    "Use Up/Down to choose, Enter to accept, and Escape to close suggestions.",
                ),
            )
    }
}

fn main() {
    gpui_kit::application().run(|cx: &mut App| {
        gpui_kit::init(cx);
        cx.spawn(async move |cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(gpui_kit::Bounds {
                    origin: point(px(100.), px(100.)),
                    size: size(px(760.), px(460.)),
                })),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                let demo = cx.new(|cx| CompletionDemo::new(window, cx));
                cx.new(|cx| Root::new(demo, window, cx))
            })
            .expect("open completion demo window");
        })
        .detach();
    });
}
