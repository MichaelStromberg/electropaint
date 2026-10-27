//! The panel's two pages and their sections, with the names it gives ep's
//! controls.

use std::cell::Cell;

use ep_core::ids;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSControlSize, NSLayoutConstraintOrientation, NSSegmentedControl, NSStackViewGravity,
    NSUserInterfaceLayoutOrientation, NSView,
};

use super::layout::{caption, empty, grid, hstack, into_view, name, ns, page, section, stack};
use super::sync::ChanRows;
use super::{Bind, PanelController};

/// The ep sliders on the shape page, with full names for ep's four-letter
/// labels.
const SLIDERS: [(u16, &str); 12] = [
    (ids::ZOOM, "Zoom"),
    (ids::TWIST, "Twist"),
    (ids::SPEED, "Speed"),
    (ids::N, "Length"),
    (ids::AMBIENT, "Ambient"),
    (ids::SHINY, "Shininess"),
    (ids::WHEEL, "Wheel"),
    (ids::SPIN, "Spin"),
    (ids::FLIP, "Flip"),
    (ids::ARM, "Arm"),
    (ids::WRIST, "Wrist"),
    (ids::SIZE, "Size"),
];
const TILES: [(u16, &str); 5] = [
    (ids::OUTLINE, "Outline"),
    (ids::FATLINE, "Fat lines"),
    (ids::FILL, "Fill"),
    (ids::TRIANGLES, "Triangles"),
    (ids::RIBBON, "Ribbons"),
];
const EFFECTS: [(u16, &str); 6] = [
    (ids::SMEAR, "Smear"),
    (ids::SMOOTH, "Smooth"),
    // ep creates this toggle but leaves it off its RGB panel (and so without
    // a key); scripts can still reach it, and so can this panel.
    (ids::FADE, "Fade"),
    (ids::BACKGROUND, "Background"),
    (ids::ZBUF, "Z-buffer"),
    (ids::LIGHT, "Lighting"),
];
const PLAYBACK: [(u16, &str); 3] = [
    (ids::STOP, "Stop"),
    (ids::FULLSCREEN, "Full screen"),
    (ids::AUTO, "Random drift"),
];
/// ep's three colour-panel dials (outa, baka, outr).
const BACKDROP: [(u16, &str); 3] = [
    (ids::ALPHA_OUT, "Outline opacity"),
    (ids::ALPHA_BACK, "Background opacity"),
    (ids::HUE_BACK_RATE, "Background hue rate"),
];

impl PanelController {
    pub(super) fn shape_page(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let position = hstack(
            mtm,
            &[
                name(mtm, "X"),
                self.slider(mtm, -0.5, 0.5, Bind::Puck(0), 160.0),
                name(mtm, "Y"),
                self.slider(mtm, -0.5, 0.5, Bind::Puck(1), 160.0),
                self.button(mtm, "Centre", Bind::PuckCentre),
            ],
        );
        page(
            mtm,
            &[
                section(mtm, "Tiles", None, self.toggle_grid(mtm, &TILES)),
                section(mtm, "Effects", None, self.toggle_grid(mtm, &EFFECTS)),
                section(mtm, "Strand", None, self.strand_grid(mtm)),
                section(mtm, "Position", None, position),
                section(mtm, "Playback", None, self.toggle_grid(mtm, &PLAYBACK)),
            ],
        )
    }

    pub(super) fn colour_page(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let backdrop = BACKDROP
            .iter()
            .map(|&(id, text)| {
                let (min, max) = self.range(id);
                vec![
                    name(mtm, text),
                    self.slider(mtm, min, max, Bind::Value(id), 200.0),
                    self.symbol_button(
                        mtm,
                        "arrow.counterclockwise",
                        "Reset",
                        Bind::Press(id + ids::DIAL_RESET),
                    ),
                ]
            })
            .collect();
        let backdrop = into_view(&*grid(mtm, backdrop));

        let mode = NSSegmentedControl::new(mtm);
        mode.setSegmentCount(2);
        mode.setLabel_forSegment(&ns("Hue"), 0);
        mode.setLabel_forSegment(&ns("Lightness"), 1);
        mode.setControlSize(NSControlSize::Small);
        let mode = self.add(&*mode, Bind::ChanMode);

        let (min, max) = self.range(ids::CHAN_RATE);
        let cycling = hstack(
            mtm,
            &[
                name(mtm, "Channel rate"),
                self.slider(mtm, min, max, Bind::Value(ids::CHAN_RATE), 300.0),
            ],
        );
        page(
            mtm,
            &[
                section(mtm, "Outline and background", None, backdrop),
                section(mtm, "Channels", Some(mode), self.channel_grid(mtm)),
                section(mtm, "Cycling", None, cycling),
            ],
        )
    }

    /// Checkboxes three to a row, each followed by its key in grey.
    fn toggle_grid(&self, mtm: MainThreadMarker, toggles: &[(u16, &str)]) -> Retained<NSView> {
        let rows = toggles
            .chunks(3)
            .map(|chunk| {
                let mut row = Vec::new();
                for &(id, text) in chunk {
                    row.push(self.checkbox(mtm, text, Bind::Press(id)));
                    row.push(caption(
                        mtm,
                        &self.key(id).map(String::from).unwrap_or_default(),
                    ));
                }
                while row.len() < 6 {
                    row.push(empty(mtm));
                }
                row
            })
            .collect();
        let g = grid(mtm, rows);
        g.setColumnSpacing(6.0);
        // Room between one toggle's key and the next toggle.
        for column in [1, 3] {
            g.columnAtIndex(column).setTrailingPadding(18.0);
        }
        into_view(&*g)
    }

    /// The ep sliders: value with its reading, the drift window, the rate,
    /// bounce and reset.
    fn strand_grid(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let mut rows = vec![
            ["", "Value", "", "Low", "High", "Rate", "", ""]
                .map(|s| caption(mtm, s))
                .to_vec(),
        ];
        for (id, text) in SLIDERS {
            let [value, low, high, rate] = self.ep_slider(mtm, id, 180.0, 70.0);
            rows.push(vec![
                name(mtm, text),
                value,
                self.readout(mtm, Bind::Readout(id)),
                low,
                high,
                rate,
                self.bounce_button(mtm, id),
                self.reset_button(mtm, id),
            ]);
        }
        into_view(&*grid(mtm, rows))
    }

    /// One column per colour channel.
    fn channel_grid(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let channels = 0..5;
        let header = std::iter::once(empty(mtm))
            .chain(
                channels
                    .clone()
                    .map(|i| caption(mtm, &format!("Channel {}", i + 1))),
            )
            .collect();
        let row = |text: &str, make: &dyn Fn(usize) -> Retained<NSView>| -> Vec<Retained<NSView>> {
            std::iter::once(name(mtm, text))
                .chain(channels.clone().map(make))
                .collect()
        };
        let mut rows: Vec<Vec<Retained<NSView>>> = vec![
            header,
            row("In use", &|i| {
                self.radio(mtm, Bind::Press(ids::active_button(i)))
            }),
            row("Colour", &|i| self.swatch(mtm, i)),
            row("Opacity", &|i| {
                self.slider(mtm, 0.0, 1.0, Bind::Value(ids::alpha_dial(i)), 90.0)
            }),
        ];
        let mut hue_rows = Vec::new();
        let mut light_rows = Vec::new();
        for (set, base) in [
            (&mut hue_rows, ids::hue_slider as fn(usize) -> u16),
            (&mut light_rows, ids::light_slider),
        ] {
            let sliders: Vec<_> = channels
                .clone()
                .map(|i| self.ep_slider(mtm, base(i), 90.0, 90.0))
                .collect();
            for (k, text) in ["Value", "Low", "High", "Rate"].into_iter().enumerate() {
                set.push(rows.len() as isize);
                rows.push(
                    std::iter::once(name(mtm, text))
                        .chain(sliders.iter().map(|s| s[k].clone()))
                        .collect(),
                );
            }
            set.push(rows.len() as isize);
            rows.push(row("", &|i| {
                into_view(&*hstack(
                    mtm,
                    &[
                        self.bounce_button(mtm, base(i)),
                        self.reset_button(mtm, base(i)),
                    ],
                ))
            }));
        }
        rows.push(row("Dwell", &|i| {
            self.slider(mtm, 0.0, 1.0, Bind::Value(ids::dwell(i)), 90.0)
        }));
        let g = grid(mtm, rows);
        g.setColumnSpacing(12.0);
        *self.ivars().chan_rows.borrow_mut() = Some(ChanRows {
            grid: g.clone(),
            hue: hue_rows,
            light: light_rows,
            hue_mode: Cell::new(None),
        });
        into_view(&*g)
    }

    pub(super) fn page_picker(
        &self,
        mtm: MainThreadMarker,
        labels: &[&str],
    ) -> Retained<NSSegmentedControl> {
        let picker = NSSegmentedControl::new(mtm);
        picker.setContentHuggingPriority_forOrientation(
            1000.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
        picker.setSegmentCount(labels.len() as isize);
        for (i, text) in labels.iter().enumerate() {
            picker.setLabel_forSegment(&ns(text), i as isize);
        }
        self.add(&*picker, Bind::Page);
        picker
    }

    /// The drawing style at the bottom left, and the buttons that act on the
    /// whole show at the bottom right.
    pub(super) fn button_bar(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let bar = stack(mtm, &[], NSUserInterfaceLayoutOrientation::Horizontal);
        let style = NSSegmentedControl::new(mtm);
        style.setSegmentCount(2);
        for (i, text) in ["Classic", "Remastered"].iter().enumerate() {
            style.setLabel_forSegment(&ns(text), i as isize);
        }
        bar.addView_inGravity(&self.add(&*style, Bind::Style), NSStackViewGravity::Leading);
        for (text, id) in [("Reset All", ids::RESET), ("Quit", ids::EXIT)] {
            bar.addView_inGravity(
                &self.button(mtm, text, Bind::Press(id)),
                NSStackViewGravity::Trailing,
            );
        }
        into_view(&*bar)
    }
}
