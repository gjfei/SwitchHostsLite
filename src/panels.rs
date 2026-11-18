//! Widths of the resizable side panels (list, hosts tree, info).

/// Width of the grab band. It is transparent until hovered.
pub(crate) const BAND: f32 = 5.;
pub(crate) const MIN_PANEL: f32 = 180.;
/// Space the main panel keeps while a side panel is dragged.
const MIN_MAIN: f32 = 320.;
/// Width of the icon bar at the start of the row.
const ICON_BAR: f32 = 40.;

/// A saved width, kept usable even if the window got narrower meanwhile.
pub(crate) fn initial_width(saved: u32) -> f32 {
    (saved as f32).max(MIN_PANEL)
}

/// The widest a side panel may get while the other one keeps its width.
pub(crate) fn max_width(total: f32, other: Option<f32>) -> f32 {
    (total - ICON_BAR - BAND - MIN_MAIN - other.map_or(0., |w| BAND + w)).max(MIN_PANEL)
}

/// Clamps both panels to the current window, shrinking the list first.
pub(crate) fn fit_widths(total: f32, list: Option<f32>, info: Option<f32>) -> (Option<f32>, Option<f32>) {
    let list = list.map(|w| w.clamp(MIN_PANEL, max_width(total, info)));
    let info = info.map(|w| w.clamp(MIN_PANEL, max_width(total, list)));
    (list, info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_shrink_to_keep_the_main_panel() {
        assert_eq!(fit_widths(1000., Some(240.), Some(240.)), (Some(240.), Some(240.)));
        assert_eq!(fit_widths(1000., Some(600.), Some(240.)), (Some(390.), Some(240.)));
        assert_eq!(fit_widths(760., Some(450.), Some(180.)), (Some(210.), Some(180.)));
        assert_eq!(fit_widths(600., Some(240.), Some(240.)), (Some(180.), Some(180.)));
        assert_eq!(fit_widths(1000., Some(100.), None), (Some(180.), None));
    }
}
