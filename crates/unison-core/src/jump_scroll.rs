//! Where the transcript ends up when a person jumps to a message that is already loaded (a reply
//! line, a pin, a search hit; docs/UX.md 5.7). One rule for every shell.
//!
//! A jump exists to show the message. If it is already on screen, comfortably (not hugging the top
//! or bottom edge), nothing scrolls and the shell only highlights it. Otherwise it is centred in the
//! visible band, and never past what the transcript can scroll to (the end of the loaded content, or
//! its start): asking a scroll view for an offset beyond its range leaves a blank gap and a view
//! that has to snap back.
//!
//! All numbers are document points, y growing downwards. The *band* is the part of the viewport the
//! reader can see: the viewport minus whatever floats over it (toolbar above, composer below).

/// A message this close to the band's edge (a fraction of the band's height, but at least
/// `MIN_MARGIN` points) counts as "at the edge", not in view.
pub const EDGE_FRACTION: f64 = 0.12;
pub const MIN_MARGIN: f64 = 40.0;
/// A change this small is not worth moving for.
const EPSILON: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// The message row.
    pub row_top: f64,
    pub row_bottom: f64,
    /// Viewport: current scroll offset (document y at the viewport's top edge) and its height.
    pub offset: f64,
    pub viewport: f64,
    /// What floats over the viewport's top and bottom edges.
    pub inset_top: f64,
    pub inset_bottom: f64,
    /// The loaded content's height.
    pub content: f64,
}

/// The offset to scroll to so the row is visible, or `None` when it already is and nothing should move.
pub fn target_offset(g: &Geometry) -> Option<f64> {
    let band = (g.viewport - g.inset_top - g.inset_bottom).max(1.0);
    let band_top = g.offset + g.inset_top;
    let band_bottom = band_top + band;
    let margin = (band * EDGE_FRACTION).max(MIN_MARGIN).min(band / 4.0);
    let height = g.row_bottom - g.row_top;

    // A row taller than the band cannot sit inside it: it is in view while its start is.
    let in_view = if height > band - 2.0 * margin {
        g.row_top >= band_top && g.row_top <= band_bottom - margin
    } else {
        g.row_top >= band_top + margin && g.row_bottom <= band_bottom - margin
    };
    if in_view {
        return None;
    }

    let wanted = if height > band - 2.0 * margin {
        g.row_top - g.inset_top - margin
    } else {
        (g.row_top + g.row_bottom) / 2.0 - g.inset_top - band / 2.0
    };
    let min = -g.inset_top;
    let max = (g.content + g.inset_bottom - g.viewport).max(min);
    let to = wanted.clamp(min, max);
    ((to - g.offset).abs() > EPSILON).then_some(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(row_top: f64, row_bottom: f64, offset: f64) -> Geometry {
        // 800 pt viewport with a 52 pt toolbar and a 100 pt composer over it: a 648 pt band.
        Geometry { row_top, row_bottom, offset, viewport: 800.0, inset_top: 52.0, inset_bottom: 100.0, content: 5000.0 }
    }

    #[test]
    fn a_row_already_in_view_does_not_scroll() {
        // Band is 2000+52 .. 2000+700 on screen: a row in the middle, and one a little off centre.
        assert_eq!(target_offset(&g(2300.0, 2360.0, 2000.0)), None);
        assert_eq!(target_offset(&g(2140.0, 2200.0, 2000.0)), None);
        assert_eq!(target_offset(&g(2540.0, 2600.0, 2000.0)), None);
    }

    #[test]
    fn a_row_at_an_edge_is_brought_in() {
        // Hugging the top of the band, hugging the bottom, partly hidden under the toolbar.
        assert!(target_offset(&g(2060.0, 2120.0, 2000.0)).is_some());
        assert!(target_offset(&g(2640.0, 2690.0, 2000.0)).is_some());
        assert!(target_offset(&g(2030.0, 2090.0, 2000.0)).is_some());
    }

    #[test]
    fn a_row_out_of_view_is_centred() {
        let to = target_offset(&g(3000.0, 3060.0, 2000.0)).unwrap();
        // Its middle lands in the middle of the band.
        let band_mid = to + 52.0 + 648.0 / 2.0;
        assert!((band_mid - 3030.0).abs() < 1.0);
    }

    #[test]
    fn never_past_the_end_or_the_start() {
        // Last message of the loaded content: the end of the content is as far as it goes.
        let mut near_end = g(4950.0, 5000.0, 100.0);
        near_end.content = 5000.0;
        let to = target_offset(&near_end).unwrap();
        assert_eq!(to, 5000.0 + 100.0 - 800.0);
        // First message: the start.
        let to = target_offset(&g(0.0, 50.0, 3000.0)).unwrap();
        assert_eq!(to, -52.0);
    }

    #[test]
    fn already_as_far_as_it_goes_does_not_scroll() {
        // The row is at the edge, but the view is already at the end: nothing can move.
        let mut at_end = g(4950.0, 5000.0, 4300.0);
        at_end.content = 5000.0;
        assert_eq!(target_offset(&at_end), None);
    }

    #[test]
    fn a_tall_row_shows_its_start() {
        // Taller than the band: in view while its top is, otherwise its top is brought just under the toolbar.
        assert_eq!(target_offset(&g(2200.0, 3400.0, 2000.0)), None);
        let to = target_offset(&g(3000.0, 4200.0, 2000.0)).unwrap();
        assert!((3000.0 - (to + 52.0)).abs() <= 648.0 / 4.0 + 1.0);
    }
}
