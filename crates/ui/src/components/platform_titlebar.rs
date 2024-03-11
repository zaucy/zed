use smallvec::SmallVec;

use gpui::{
    div, prelude::FluentBuilder, px, AnyElement, Div, Element, Fill, InteractiveElement,
    IntoElement, ParentElement, RenderOnce, Rgba, Styled, WindowBounds, WindowContext,
};

use crate::{h_flex, ButtonLike};

#[derive(IntoElement)]
pub struct PlatformTitlebar {
    titlebar_bg: Option<Fill>,
    children: SmallVec<[AnyElement; 2]>,
}

impl PlatformTitlebar {
    fn render_caption_buttons(_cx: &mut WindowContext) -> impl Element {
        div()
            .id("caption-buttons-windows")
            .flex()
            .flex_row()
            .justify_center()
            .content_stretch()
            .font("Segoe Fluent Icons")
            .children(vec![
                div().h_full().justify_center().w_16().child("\u{e921}"), // minimize
                div().h_full().justify_center().w_16().child("\u{e922}"), // maximize
                div()
                    .h_full()
                    .justify_center()
                    .w_16()
                    .hover(|style| {
                        style.bg(Rgba {
                            r: 0.8,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        })
                    })
                    .child("\u{e8bb}"), // close
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
            .h(cx.titlebar_height())
            .neg_my_1() // TODO: figure out why this is needed for the titlebar to be snug
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
