//! Where pages are on screen.
//!
//! Pages are stacked top to bottom and centred on a common vertical axis.
//! Everything here is whole pixels at the current zoom: a page is drawn at an
//! integer position and its tiles map one to one onto screen pixels, which
//! keeps text as sharp as Typst rendered it.

use zetwerk_render::PixelRegion;

/// Space between pages and around the document, in points.
const GAP_PT: f64 = 10.0;

/// Zoom limits in pixels per point. Below the minimum a page is a thumbnail;
/// above the maximum a single page needs hundreds of megabytes of tiles.
pub const MIN_ZOOM: f64 = 0.1;
pub const MAX_ZOOM: f64 = 16.0;

/// The pages of a document laid out at one zoom level, in pixels.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    pub pages: Vec<PageBox>,
    /// Width of the widest page plus the gap on both sides.
    pub width: u32,
    /// Height of all pages and the gaps around them.
    pub height: u32,
}

/// One page in a [`Layout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageBox {
    /// Distance from the top of the document.
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Layout {
    /// Lays out pages of the given pixel sizes at `zoom` pixels per point.
    #[must_use]
    pub fn new(sizes: impl IntoIterator<Item = (u32, u32)>, zoom: f64) -> Self {
        let gap = gap_px(zoom);
        let mut y = gap;
        let mut widest = 0;
        let pages = sizes
            .into_iter()
            .map(|(width, height)| {
                let page = PageBox { y, width, height };
                y += height + gap;
                widest = widest.max(width);
                page
            })
            .collect();
        Self {
            pages,
            width: widest + 2 * gap,
            height: y,
        }
    }

    /// Lays out pages of the given pixel sizes as slides: each alone in a
    /// stretch as high as a view of `view_height` pixels (or as itself,
    /// if it is higher), in the middle of it. A view at the top of such a
    /// stretch shows that page and nothing of its neighbours.
    #[must_use]
    pub fn slides(
        sizes: impl IntoIterator<Item = (u32, u32)>,
        zoom: f64,
        view_height: u32,
    ) -> Self {
        let gap = gap_px(zoom);
        let mut y = 0;
        let mut widest = 0;
        let pages = sizes
            .into_iter()
            .map(|(width, height)| {
                let stretch = view_height.max(height + 2 * gap);
                let page = PageBox {
                    y: y + (stretch - height) / 2,
                    width,
                    height,
                };
                y += stretch;
                widest = widest.max(width);
                page
            })
            .collect();
        Self {
            pages,
            width: widest + 2 * gap,
            height: y,
        }
    }

    /// The page showing at `y` pixels from the top of the document, or the
    /// nearest one if `y` falls into a gap.
    #[must_use]
    pub fn page_at(&self, y: i64) -> Option<usize> {
        let below = self
            .pages
            .partition_point(|page| i64::from(page.y + page.height) <= y);
        (!self.pages.is_empty()).then(|| below.min(self.pages.len() - 1))
    }
}

fn gap_px(zoom: f64) -> u32 {
    (GAP_PT * zoom).round().max(1.0) as u32
}

/// Which part of the document the window shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Device pixels per typographic point.
    pub zoom: f64,
    /// Distance of the view's top edge from the top of the document, in
    /// points. Kept in points so that it means the same place at any zoom.
    pub top: f64,
    /// Horizontal offset of the view's centre from the pages' centre axis,
    /// in points. Zero shows pages centred.
    pub center: f64,
    /// Size of the area pages are drawn in, in pixels.
    pub width: u32,
    pub height: u32,
}

/// A page, or part of one, that is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisiblePage {
    pub index: usize,
    /// Screen position of the page's top-left corner. May be off-screen.
    pub x: i64,
    pub y: i64,
    /// The part of the page inside the view, in page pixels.
    pub region: PixelRegion,
}

impl Viewport {
    fn top_px(&self) -> i64 {
        (self.top * self.zoom).round() as i64
    }

    fn center_px(&self) -> i64 {
        (self.center * self.zoom).round() as i64
    }

    /// The pages of `layout` that intersect the view.
    #[must_use]
    pub fn visible(&self, layout: &Layout) -> Vec<VisiblePage> {
        let (top, center) = (self.top_px(), self.center_px());
        let (view_width, view_height) = (i64::from(self.width), i64::from(self.height));
        layout
            .pages
            .iter()
            .enumerate()
            .filter_map(|(index, page)| {
                let x = view_width / 2 - i64::from(page.width / 2) - center;
                let y = i64::from(page.y) - top;
                let (left, right) = (x.max(0), (x + i64::from(page.width)).min(view_width));
                let (upper, lower) = (y.max(0), (y + i64::from(page.height)).min(view_height));
                if left >= right || upper >= lower {
                    return None;
                }
                let region = PixelRegion {
                    x: u32::try_from(left - x).ok()?,
                    y: u32::try_from(upper - y).ok()?,
                    width: u32::try_from(right - left).ok()?,
                    height: u32::try_from(lower - upper).ok()?,
                };
                Some(VisiblePage {
                    index,
                    x,
                    y,
                    region,
                })
            })
            .collect()
    }

    /// The page at the vertical middle of the view.
    #[must_use]
    pub fn current_page(&self, layout: &Layout) -> Option<usize> {
        layout.page_at(self.top_px() + i64::from(self.height / 2))
    }

    /// Scrolls by whole pixels and keeps the view inside the document.
    pub fn scroll_by(&mut self, dx: f64, dy: f64, layout: &Layout) {
        self.center += dx / self.zoom;
        self.top += dy / self.zoom;
        self.clamp(layout);
    }

    /// Puts the top of page `index` at the top of the view.
    pub fn scroll_to_page(&mut self, index: usize, layout: &Layout) {
        if let Some(page) = layout.pages.get(index) {
            let gap = f64::from(gap_px(self.zoom));
            self.top = (f64::from(page.y) - gap) / self.zoom;
            self.clamp(layout);
        }
    }

    /// Puts page `index` in the middle of the view, top to bottom.
    pub fn center_page(&mut self, index: usize, layout: &Layout) {
        if let Some(page) = layout.pages.get(index) {
            let middle = f64::from(page.y) + f64::from(page.height) / 2.0;
            self.top = (middle - f64::from(self.height) / 2.0) / self.zoom;
            self.clamp(layout);
        }
    }

    /// Keeps the view inside the document: no scrolling past its ends, and
    /// no horizontal offset while the pages fit the view.
    pub fn clamp(&mut self, layout: &Layout) {
        let max_top = f64::from(layout.height.saturating_sub(self.height)) / self.zoom;
        self.top = self.top.clamp(0.0, max_top);
        let max_center = f64::from(layout.width.saturating_sub(self.width)) / 2.0 / self.zoom;
        self.center = self.center.clamp(-max_center, max_center);
    }

    /// Changes the zoom so that the document point under the screen position
    /// `anchor` stays where it is. The caller lays the document out again
    /// and then calls [`Viewport::clamp`].
    pub fn zoom_about(&mut self, zoom: f64, anchor: (f64, f64)) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let from_center = anchor.0 - f64::from(self.width) / 2.0;
        self.center += from_center / self.zoom - from_center / zoom;
        self.top += anchor.1 / self.zoom - anchor.1 / zoom;
        self.zoom = zoom;
    }

    /// The zoom at which a page `page_width` points wide fills the view's
    /// width, leaving the usual gap on both sides.
    #[must_use]
    pub fn fit_width(&self, page_width: f64) -> f64 {
        (f64::from(self.width) / (page_width + 2.0 * GAP_PT)).clamp(MIN_ZOOM, MAX_ZOOM)
    }

    /// The zoom at which a page of `page_width` by `page_height` points
    /// is in the view whole, with the usual gap around it.
    #[must_use]
    pub fn fit_page(&self, page_width: f64, page_height: f64) -> f64 {
        let by_height = f64::from(self.height) / (page_height + 2.0 * GAP_PT);
        self.fit_width(page_width)
            .min(by_height)
            .clamp(MIN_ZOOM, MAX_ZOOM)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport(zoom: f64) -> Viewport {
        Viewport {
            zoom,
            top: 0.0,
            center: 0.0,
            width: 800,
            height: 600,
        }
    }

    fn a4_layout(pages: usize, zoom: f64) -> Layout {
        let size = ((595.0 * zoom) as u32, (842.0 * zoom) as u32);
        Layout::new(std::iter::repeat_n(size, pages), zoom)
    }

    #[test]
    fn pages_are_stacked_with_gaps() {
        let layout = Layout::new([(100, 200), (300, 50)], 1.0);
        assert_eq!(
            layout.pages[0],
            PageBox {
                y: 10,
                width: 100,
                height: 200
            }
        );
        assert_eq!(
            layout.pages[1],
            PageBox {
                y: 220,
                width: 300,
                height: 50
            }
        );
        assert_eq!((layout.width, layout.height), (320, 280));
    }

    #[test]
    fn page_at_picks_the_nearest_page() {
        let layout = Layout::new([(100, 200), (100, 200)], 1.0);
        assert_eq!(layout.page_at(-50), Some(0));
        assert_eq!(layout.page_at(100), Some(0));
        assert_eq!(layout.page_at(215), Some(1));
        assert_eq!(layout.page_at(10_000), Some(1));
        assert_eq!(Layout::default().page_at(0), None);
    }

    #[test]
    fn only_pages_in_view_are_visible_and_clipped() {
        let layout = a4_layout(3, 1.0);
        let mut view = viewport(1.0);
        let visible = view.visible(&layout);
        assert_eq!(visible.len(), 1);
        // Centred: (800 - 595) / 2 with the page's half width rounded down.
        assert_eq!((visible[0].x, visible[0].y), (103, 10));
        assert_eq!(
            visible[0].region,
            PixelRegion {
                x: 0,
                y: 0,
                width: 595,
                height: 590
            }
        );

        view.scroll_by(0.0, 600.0, &layout);
        let visible = view.visible(&layout);
        assert_eq!(
            visible.iter().map(|page| page.index).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(visible[0].region.y, 590);
        assert_eq!(visible[0].region.height, 842 - 590);
    }

    #[test]
    fn scrolling_stops_at_the_document_ends() {
        let layout = a4_layout(2, 1.0);
        let mut view = viewport(1.0);
        view.scroll_by(0.0, -500.0, &layout);
        assert_eq!(view.top, 0.0);
        view.scroll_by(0.0, 1e9, &layout);
        assert_eq!(view.top, f64::from(layout.height - 600));
        // The page is narrower than the view: no horizontal scrolling.
        view.scroll_by(300.0, 0.0, &layout);
        assert_eq!(view.center, 0.0);
    }

    #[test]
    fn zooming_keeps_the_anchor_in_place() {
        let mut view = viewport(1.0);
        view.top = 100.0;
        // The document point under screen position (600, 300).
        let before = (
            view.center + (600.0 - 400.0) / view.zoom,
            view.top + 300.0 / view.zoom,
        );
        view.zoom_about(2.5, (600.0, 300.0));
        let after = (
            view.center + (600.0 - 400.0) / view.zoom,
            view.top + 300.0 / view.zoom,
        );
        assert!((before.0 - after.0).abs() < 1e-9 && (before.1 - after.1).abs() < 1e-9);
        assert_eq!(view.zoom, 2.5);
    }

    #[test]
    fn zoom_is_limited() {
        let mut view = viewport(1.0);
        view.zoom_about(1000.0, (0.0, 0.0));
        assert_eq!(view.zoom, MAX_ZOOM);
        view.zoom_about(0.0, (0.0, 0.0));
        assert_eq!(view.zoom, MIN_ZOOM);
    }

    #[test]
    fn fit_width_fills_the_view() {
        let view = viewport(1.0);
        let zoom = view.fit_width(595.0);
        let layout = a4_layout(1, zoom);
        assert!(
            layout.width <= 800 && layout.width >= 796,
            "layout is {} wide",
            layout.width
        );
    }

    #[test]
    fn current_page_follows_scrolling() {
        let layout = a4_layout(3, 1.0);
        let mut view = viewport(1.0);
        assert_eq!(view.current_page(&layout), Some(0));
        view.scroll_to_page(2, &layout);
        assert_eq!(view.current_page(&layout), Some(2));
        assert_eq!(view.visible(&layout)[0].y, 10);
    }
}
