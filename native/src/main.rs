use gpui::*;
use gpui_component::{button::Button, Root};

struct Hello;

impl Render for Hello {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .items_center()
            .justify_center()
            .bg(rgb(0x1f1f1f))
            .text_color(rgb(0xcccccc))
            .child("Review Tool (native) spike")
            .child(Button::new("ok").label("It works"))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        gpui_component::init(cx);
        cx.open_window(WindowOptions::default(), |window, cx| {
            let view = cx.new(|_| Hello);
            cx.new(|cx| Root::new(view, window, cx))
        })
        .unwrap();
        cx.activate(true);
    });
}
