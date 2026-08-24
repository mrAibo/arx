//! #16 v1: the ONE deterministic split-geometry authority.
//!
//! Pure presentation math — no state, no runtime, no provider/pane authority.
//! Render AND mouse hit-testing MUST both call `split_rects` here; no copied
//! half-split arithmetic elsewhere.

use super::*;
use arx::app::SplitOrientation;

/// Which of the two same-location subviews a point/cursor belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SplitSection {
    Primary,
    Secondary,
}

/// Resolved rectangles for one outer pane. `secondary == None` when the split
/// is disabled or the terminal axis is too small to show two views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SplitRects {
    pub primary: Rect,
    pub secondary: Option<Rect>,
}

/// Divide one outer pane into primary/secondary same-location subviews.
///
/// - disabled => primary = area, secondary = None
/// - axis length < 2 => primary = area, secondary = None (no panic; split
///   state is preserved and re-applied after a terminal resize)
/// - otherwise: ratio clamped to 20..=80; primary axis length =
///   floor(axis * ratio / 100) clamped to 1..=axis-1; exact remainder goes to
///   secondary (union equals the original area on the split axis).
pub(super) fn split_rects(
    area: Rect,
    enabled: bool,
    orientation: SplitOrientation,
    ratio: u16,
) -> SplitRects {
    if !enabled {
        return SplitRects {
            primary: area,
            secondary: None,
        };
    }
    let vertical = orientation == SplitOrientation::Vertical;
    let axis = if vertical { area.width } else { area.height };
    if axis < 2 {
        return SplitRects {
            primary: area,
            secondary: None,
        };
    }
    let ratio = ratio.clamp(arx::app::SPLIT_RATIO_MIN, arx::app::SPLIT_RATIO_MAX);
    let primary_len = (((axis as u32) * (ratio as u32) / 100) as u16).clamp(1, axis - 1);
    let secondary_len = axis - primary_len;
    let (primary, secondary) = if vertical {
        (
            Rect::new(area.x, area.y, primary_len, area.height),
            Some(Rect::new(
                area.x + primary_len,
                area.y,
                secondary_len,
                area.height,
            )),
        )
    } else {
        (
            Rect::new(area.x, area.y, area.width, primary_len),
            Some(Rect::new(
                area.x,
                area.y + primary_len,
                area.width,
                secondary_len,
            )),
        )
    };
    SplitRects { primary, secondary }
}

/// Resolve which section (if any) contains the pointer. Returns the section
/// and ITS rect so callers can compute row/column relative to that subview.
pub(super) fn section_at_point(
    rects: &SplitRects,
    column: u16,
    row: u16,
) -> Option<(SplitSection, Rect)> {
    let inside =
        |r: Rect| column >= r.x && column < r.x + r.width && row >= r.y && row < r.y + r.height;
    if inside(rects.primary) {
        return Some((SplitSection::Primary, rects.primary));
    }
    if let Some(secondary) = rects.secondary
        && inside(secondary)
    {
        return Some((SplitSection::Secondary, secondary));
    }
    None
}

/// One rendered subview surface plus its active-border flag.
pub(super) type SurfacePlan = (Rect, bool);
/// Primary plan + optional secondary plan (None => primary-only render).
pub(super) type RenderPlan = Option<(SurfacePlan, Option<SurfacePlan>)>;

/// #16 review fix: derive the RENDER PLAN from geometry + focus state.
///
/// `secondary == None` (axis < 2) means primary-only: exactly one surface,
/// always active when the outer pane is active, regardless of the hidden
/// split_active. Pure derivation — no state mutation.
pub(super) fn render_plan(rects: SplitRects, outer_active: bool, split_active: bool) -> RenderPlan {
    let secondary = rects.secondary?;
    Some((
        (rects.primary, outer_active && !split_active),
        Some((secondary, outer_active && split_active)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    #[test]
    fn r16_disabled_split_returns_full_area() {
        let r = split_rects(rect(100, 10), false, SplitOrientation::Vertical, 50);
        assert_eq!(r.primary, rect(100, 10));
        assert!(r.secondary.is_none());
    }

    #[test]
    fn r16_vertical_50_50() {
        let r = split_rects(rect(100, 10), true, SplitOrientation::Vertical, 50);
        assert_eq!(r.primary.width, 50);
        assert_eq!(r.secondary.unwrap().width, 50);
        assert_eq!(r.secondary.unwrap().x, 50);
    }

    #[test]
    fn r16_vertical_20_80_and_80_20() {
        let lo = split_rects(rect(100, 10), true, SplitOrientation::Vertical, 20);
        assert_eq!(lo.primary.width, 20);
        assert_eq!(lo.secondary.unwrap().width, 80);
        let hi = split_rects(rect(100, 10), true, SplitOrientation::Vertical, 80);
        assert_eq!(hi.primary.width, 80);
        assert_eq!(hi.secondary.unwrap().width, 20);
    }

    #[test]
    fn r16_ratio_clamped_into_bounds() {
        let r = split_rects(rect(100, 10), true, SplitOrientation::Vertical, 5);
        assert_eq!(r.primary.width, 20);
        let r = split_rects(rect(100, 10), true, SplitOrientation::Vertical, 99);
        assert_eq!(r.primary.width, 80);
    }

    #[test]
    fn r16_odd_width_exact_union() {
        let r = split_rects(rect(101, 10), true, SplitOrientation::Vertical, 50);
        assert_eq!(r.primary.width + r.secondary.unwrap().width, 101);
        assert_eq!(
            r.primary.width + r.secondary.unwrap().width,
            101 // union equals original on the split axis
        );
        assert_eq!(r.secondary.unwrap().x, r.primary.width);
    }

    #[test]
    fn r16_horizontal_uses_height() {
        let r = split_rects(rect(40, 30), true, SplitOrientation::Horizontal, 50);
        assert_eq!(r.primary.height, 15);
        assert_eq!(r.secondary.unwrap().height, 15);
        assert_eq!(r.secondary.unwrap().y, 15);
        assert_eq!(r.primary.width, 40);
    }

    #[test]
    fn r16_degenerate_axes_safe() {
        // axis length 0 / 1 / 2 must not panic, overflow, or overlap.
        let r = split_rects(rect(0, 10), true, SplitOrientation::Vertical, 50);
        assert!(r.secondary.is_none());
        assert_eq!(r.primary.width, 0);
        let r = split_rects(rect(1, 10), true, SplitOrientation::Vertical, 50);
        assert!(r.secondary.is_none());
        assert_eq!(r.primary.width, 1);
        let r = split_rects(rect(2, 10), true, SplitOrientation::Vertical, 50);
        assert_eq!(r.secondary.unwrap().width, 1);
        assert_eq!(r.primary.width, 1);

        let r = split_rects(rect(10, 1), true, SplitOrientation::Horizontal, 50);
        assert!(r.secondary.is_none());
        assert_eq!(r.primary.height, 1);
        let r = split_rects(rect(10, 0), true, SplitOrientation::Horizontal, 50);
        assert!(r.secondary.is_none());
    }

    #[test]
    fn r16_section_at_point_resolves_both_sections() {
        let rects = split_rects(rect(100, 10), true, SplitOrientation::Horizontal, 50);
        let (section, _) = section_at_point(&rects, 5, 3).unwrap();
        assert_eq!(section, SplitSection::Primary);
        let (section, sec_rect) = section_at_point(&rects, 5, 8).unwrap();
        assert_eq!(section, SplitSection::Secondary);
        assert_eq!(sec_rect.y, 5);
        assert!(section_at_point(&rects, 200, 200).is_none());
    }
}

#[cfg(test)]
mod r16fix_tests {
    use super::*;

    /// Behavioral contract: with split_active=true but a degenerate split axis
    /// (secondary=None), the visible render plan is PRIMARY-only — the hidden
    /// secondary cursor/focus can never overwrite the primary surface.
    #[test]
    fn narrow_split_renders_primary_only_even_when_secondary_has_focus() {
        let area = Rect::new(0, 0, 1, 30); // width 1 => vertical axis < 2
        let rects = split_rects(area, true, SplitOrientation::Vertical, 50);
        assert!(
            rects.secondary.is_none(),
            "precondition: axis<2 hides secondary"
        );

        // split_active=true & hidden: None plan == primary-only render.
        assert!(
            render_plan(rects, true, true).is_none(),
            "hidden secondary must not be rendered"
        );
        // The visible primary surface is the full pane area (from geometry).
        assert_eq!(rects.primary, area);

        // And the same for horizontal orientation.
        let rects = split_rects(
            Rect::new(0, 0, 40, 1),
            true,
            SplitOrientation::Horizontal,
            50,
        );
        assert!(render_plan(rects, true, true).is_none());
        assert_eq!(rects.secondary, None);
    }

    /// Both-visible case: focus follows split_active exactly as before.
    #[test]
    fn wide_split_focus_follows_split_active() {
        let rects = split_rects(
            Rect::new(0, 0, 100, 10),
            true,
            SplitOrientation::Vertical,
            50,
        );
        let (primary, secondary) = render_plan(rects, true, false).unwrap();
        assert!(primary.1 && !secondary.unwrap().1);
        let (primary, secondary) = render_plan(rects, true, true).unwrap();
        assert!(!primary.1 && secondary.unwrap().1);
        // Inactive outer pane: neither subview shows active borders.
        let (primary, secondary) = render_plan(rects, false, true).unwrap();
        assert!(!primary.1 && !secondary.unwrap().1);
    }
}
