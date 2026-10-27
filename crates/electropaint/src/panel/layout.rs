//! Building blocks for the panel's layout: labels, grids, stacks, and the
//! settings-style pages, sections and cards.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, Message};
use objc2_app_kit::{
    NSBox, NSBoxType, NSColor, NSFont, NSGridCell, NSGridCellPlacement, NSGridRowAlignment,
    NSGridView, NSLayoutAttribute, NSLayoutConstraint, NSLayoutConstraintOrientation, NSStackView,
    NSStackViewGravity, NSTextField, NSTitlePosition, NSUserInterfaceLayoutOrientation, NSView,
    NSWindowOrderingMode,
};
use objc2_foundation::{NSArray, NSEdgeInsets, NSString};

pub(super) fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// Any view, as an `NSView`.
pub(super) fn into_view(v: &(impl AsRef<NSView> + ?Sized)) -> Retained<NSView> {
    v.as_ref().retain()
}

pub(super) fn text(mtm: MainThreadMarker, s: &str) -> Retained<NSTextField> {
    NSTextField::labelWithString(&ns(s), mtm)
}

/// A row or field name.
pub(super) fn name(mtm: MainThreadMarker, s: &str) -> Retained<NSView> {
    into_view(&*text(mtm, s))
}

/// Small secondary text: column headings and key hints.
pub(super) fn caption(mtm: MainThreadMarker, s: &str) -> Retained<NSView> {
    let t = text(mtm, s);
    t.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    t.setTextColor(Some(&NSColor::secondaryLabelColor()));
    into_view(&*t)
}

pub(super) fn empty(mtm: MainThreadMarker) -> Retained<NSView> {
    NSGridCell::emptyContentView(mtm)
}

pub(super) fn grid(
    mtm: MainThreadMarker,
    rows: Vec<Vec<Retained<NSView>>>,
) -> Retained<NSGridView> {
    let rows: Vec<Retained<NSArray<NSView>>> = rows
        .into_iter()
        .map(|r| NSArray::from_retained_slice(&r))
        .collect();
    let g = NSGridView::gridViewWithViews(&NSArray::from_retained_slice(&rows), mtm);
    g.setRowSpacing(6.0);
    g.setColumnSpacing(10.0);
    // Labels level with the controls beside them.
    g.setRowAlignment(NSGridRowAlignment::None);
    g.setYPlacement(NSGridCellPlacement::Center);
    // Keep rows at their natural height when the section is taller.
    g.setContentHuggingPriority_forOrientation(999.0, NSLayoutConstraintOrientation::Vertical);
    g
}

pub(super) fn stack(
    mtm: MainThreadMarker,
    views: &[Retained<NSView>],
    o: NSUserInterfaceLayoutOrientation,
) -> Retained<NSStackView> {
    let s = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(views), mtm);
    s.setOrientation(o);
    s
}

pub(super) fn hstack(mtm: MainThreadMarker, views: &[Retained<NSView>]) -> Retained<NSView> {
    let s = stack(mtm, views, NSUserInterfaceLayoutOrientation::Horizontal);
    s.setSpacing(10.0);
    into_view(&*s)
}

/// Makes `child` as wide as `parent`.
pub(super) fn fill_width(child: &NSView, parent: &NSView) {
    child
        .widthAnchor()
        .constraintEqualToAnchor(&parent.widthAnchor())
        .setActive(true);
}

/// A page: its sections one under another, all the same width.
pub(super) fn page(mtm: MainThreadMarker, sections: &[Retained<NSView>]) -> Retained<NSView> {
    let s = stack(mtm, sections, NSUserInterfaceLayoutOrientation::Vertical);
    s.setAlignment(NSLayoutAttribute::Leading);
    s.setSpacing(18.0);
    for section in sections {
        fill_width(section, &s);
    }
    into_view(&*s)
}

/// A section in the System Settings style: a heading, with an optional
/// control at its trailing end, over a rounded card holding `content`.
pub(super) fn section(
    mtm: MainThreadMarker,
    title: &str,
    accessory: Option<Retained<NSView>>,
    content: Retained<NSView>,
) -> Retained<NSView> {
    let label = text(mtm, title);
    label.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, 0.3)));
    let heading = stack(
        mtm,
        &[into_view(&*label)],
        NSUserInterfaceLayoutOrientation::Horizontal,
    );
    if let Some(accessory) = accessory {
        heading.addView_inGravity(&accessory, NSStackViewGravity::Trailing);
    }
    let card = card(mtm, content);
    let heading = into_view(&*heading);
    let s = stack(
        mtm,
        &[heading.clone(), card.clone()],
        NSUserInterfaceLayoutOrientation::Vertical,
    );
    s.setAlignment(NSLayoutAttribute::Leading);
    s.setSpacing(6.0);
    fill_width(&heading, &s);
    fill_width(&card, &s);
    into_view(&*s)
}

/// `content` on AppKit's group box background (which follows light and dark
/// mode), at its natural width at the leading edge however wide the card is.
/// Without the high hugging, the content's default hugging ties with the
/// stack view's pull towards its edges (both at low priority), and Auto
/// Layout settles the tie differently from run to run, spreading a grid's
/// columns out.
pub(super) fn card(mtm: MainThreadMarker, content: Retained<NSView>) -> Retained<NSView> {
    content
        .setContentHuggingPriority_forOrientation(999.0, NSLayoutConstraintOrientation::Horizontal);
    let s = stack(
        mtm,
        &[content],
        NSUserInterfaceLayoutOrientation::Horizontal,
    );
    s.setEdgeInsets(NSEdgeInsets {
        top: 12.0,
        left: 14.0,
        bottom: 12.0,
        right: 14.0,
    });
    let background = NSBox::new(mtm);
    background.setBoxType(NSBoxType::Primary);
    background.setTitlePosition(NSTitlePosition::NoTitle);
    background.setTranslatesAutoresizingMaskIntoConstraints(false);
    s.addSubview_positioned_relativeTo(&background, NSWindowOrderingMode::Below, None);
    pin_edges(&background, &s);
    into_view(&*s)
}

/// Activates `constraints` together.
pub(super) fn activate(constraints: &[Retained<NSLayoutConstraint>]) {
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(constraints));
}

/// Pins `child`'s edges to `parent`'s.
pub(super) fn pin_edges(child: &NSView, parent: &NSView) {
    activate(&[
        child
            .leadingAnchor()
            .constraintEqualToAnchor(&parent.leadingAnchor()),
        child
            .trailingAnchor()
            .constraintEqualToAnchor(&parent.trailingAnchor()),
        child
            .topAnchor()
            .constraintEqualToAnchor(&parent.topAnchor()),
        child
            .bottomAnchor()
            .constraintEqualToAnchor(&parent.bottomAnchor()),
    ]);
}
