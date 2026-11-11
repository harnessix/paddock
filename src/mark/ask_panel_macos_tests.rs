//! The colors of the firewall panel in both appearances, on a Mac: the
//! semantic colors that the panel uses resolve to text that passes WCAG AA
//! on the window background, in Aqua and in Dark Aqua.

use crate::mark::panels::{contrast, AA};
use block2::RcBlock;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSColor, NSColorSpace,
};
use std::cell::Cell;

/// The sRGB components of `color` resolved in the appearance `name`.
fn resolved(
    name: &objc2_app_kit::NSAppearanceName,
    color: fn() -> objc2::rc::Retained<NSColor>,
) -> (f64, f64, f64, f64) {
    let appearance = NSAppearance::appearanceNamed(name).expect("a system appearance");
    let out = Cell::new((0.0, 0.0, 0.0, 0.0));
    let block = RcBlock::new(|| {
        let c = color()
            .colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())
            .expect("an sRGB color");
        out.set((
            c.redComponent(),
            c.greenComponent(),
            c.blueComponent(),
            c.alphaComponent(),
        ));
    });
    appearance.performAsCurrentDrawingAppearance(&block);
    out.get()
}

#[test]
fn the_text_of_the_panel_passes_aa_in_light_and_dark() {
    // SAFETY: constants of AppKit.
    let names = unsafe { [NSAppearanceNameAqua, NSAppearanceNameDarkAqua] };
    for name in names {
        let (r, g, b, _) = resolved(name, NSColor::windowBackgroundColor);
        let text = resolved(name, NSColor::labelColor);
        let ratio = contrast(text, (r, g, b));
        assert!(
            ratio >= AA,
            "{name}: labelColor on windowBackgroundColor is {ratio:.2}:1"
        );
        let secondary = contrast(resolved(name, NSColor::secondaryLabelColor), (r, g, b));
        eprintln!("{name}: labelColor {ratio:.2}:1, secondaryLabelColor {secondary:.2}:1");
    }
}

#[test]
fn new_answer_tags_round_trip() {
    use crate::broker::ConnectAnswer;
    for answer in [ConnectAnswer::AllowSite, ConnectAnswer::AllowEverything] {
        let tag = super::tag_of(answer);
        assert!(tag >= 0);
        assert_eq!(ConnectAnswer::ALL[tag as usize], answer);
    }
}
