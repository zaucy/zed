use smallvec::SmallVec;

use gpui::{
    div,
    prelude::FluentBuilder,
    px, AnyElement, Div, Element, Fill, Hsla, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, Rgba, Styled,
    WindowAppearance::{Dark, Light, VibrantDark, VibrantLight},
    WindowBounds, WindowContext,
};

use crate::{h_flex, ButtonLike};

#[derive(IntoElement)]
pub struct PlatformTitlebar {
    titlebar_bg: Option<Fill>,
    children: SmallVec<[AnyElement; 2]>,
}

impl PlatformTitlebar {
    fn render_caption_buttons(cx: &mut WindowContext) -> impl Element {
        let close_btn_hover_color = Rgba {
            r: 232.0 / 255.0,
            g: 17.0 / 255.0,
            b: 32.0 / 255.0,
            a: 1.0,
        };

        let btn_hover_color = match cx.appearance() {
            Light | VibrantLight => Rgba {
                r: 0.9,
                g: 0.9,
                b: 0.9,
                a: 0.5,
            },
            Dark | VibrantDark => Rgba {
                r: 0.1,
                g: 0.1,
                b: 0.1,
                a: 0.1,
            },
        };

        fn windows_caption_btn(icon_text: &'static str, hover_color: Rgba) -> impl IntoElement {
            let mut active_color = hover_color.clone();
            active_color.a -= 0.2;
            div()
                .h_full()
                .justify_center()
                .content_center()
                .items_center()
                .w_16()
                .hover(|style| style.bg(hover_color))
                // .active(|style| style.bg(pressed_color))
                .child(icon_text)
        }

        div()
            .id("caption-buttons-windows")
            .flex()
            .flex_row()
            .justify_center()
            .content_stretch()
            .max_h(cx.titlebar_height())
            .min_h(cx.titlebar_height())
            .font("Segoe Fluent Icons")
            .children(vec![
                windows_caption_btn("\u{e921}", btn_hover_color), // minimize
                windows_caption_btn("\u{e922}", btn_hover_color), // maximize
                windows_caption_btn("\u{e8bb}", close_btn_hover_color), // close
            ])
    }

    /// Sets the background color of titlebar.
    pub fn titlebar_bg<F>(mut self, fill: F) -> Self
    where
        F: Into<Fill>,
        Self: Sized,
    {
        self.titlebar_bg = Some(fill.into());
        self
    }
}

/// .
pub fn platform_titlebar() -> PlatformTitlebar {
    PlatformTitlebar {
        titlebar_bg: None,
        children: SmallVec::new(),
    }
}

impl RenderOnce for PlatformTitlebar {
    fn render(self, cx: &mut WindowContext) -> impl IntoElement {
        h_flex()
            .id("titlebar")
            .w_full()
            .max_h(cx.titlebar_height())
            .min_h(cx.titlebar_height())
            .map(|mut this| {
                this.style().background = self.titlebar_bg;
                if matches!(cx.window_bounds(), WindowBounds::Fullscreen) {
                    return this;
                }

                if cfg!(macos) {
                    // Use pixels here instead of a rem-based size because the macOS traffic
                    // lights are a static size, and don't scale with the rest of the UI.
                    this.pl(px(80.))
                } else {
                    this
                }
            })
            .content_stretch()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .w_full()
                    .id("titlebar-content")
                    .children(self.children),
            )
            .map(|this| {
                if cfg!(target_os = "windows") {
                    this.child(PlatformTitlebar::render_caption_buttons(cx))
                } else {
                    this
                }
            })
    }
}

impl ParentElement for PlatformTitlebar {
    fn extend(&mut self, elements: impl Iterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}
