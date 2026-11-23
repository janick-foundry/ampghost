use std::rc::Rc;

use adw::prelude::*;
use ampghost_engine::{ChainOutcome, Command};
use ampghost_params::ParamTable;
use ampghost_protocol::KATANA_50_MK2;
use gtk::glib;

use crate::blocks::{self, BlockSpec, Variant};
use crate::i18n::{tr, trf};
use crate::jobs::Job;
use crate::params_view::Edit;
use crate::report;

use super::Ui;
use super::detail::{SCROLLBAR_ROOM, section_heading};
use super::model::{hex, writable};

pub(super) const TOGGLE_FADE: u32 = 250;

pub(super) struct Card {
    pub(super) key: &'static str,
    pub(super) root: gtk::Overlay,
    pub(super) dot: gtk::Box,
    pub(super) type_label: gtk::Label,
    pub(super) power: Option<gtk::ToggleButton>,
}

pub(super) fn build_blocks_row(
    chain_picker: &gtk::DropDown,
) -> (gtk::Box, gtk::Box, gtk::Box, gtk::Box) {
    let (chain, chain_box) = node_strip(&tr("Signal Chain"), Some(chain_picker.upcast_ref()));
    let (output, extras_box) = node_strip(&tr("Output"), None);
    (chain, chain_box, output, extras_box)
}

pub(super) fn chain_picker(table: &ParamTable) -> gtk::DropDown {
    let labels = KATANA_50_MK2
        .chain_patterns
        .iter()
        .map(|pattern| blocks::pattern_label(table, pattern))
        .collect::<Vec<_>>();
    let names = labels.iter().map(String::as_str).collect::<Vec<_>>();
    let picker = gtk::DropDown::from_strings(&names);
    picker.set_halign(gtk::Align::End);
    picker.set_margin_end(ROW_INSET);
    picker.set_tooltip_text(Some(&tr("Signal chain order")));
    picker.update_property(&[gtk::accessible::Property::Label(&tr("Signal Chain Order"))]);
    picker.add_css_class("flat");
    picker
}

pub(super) fn node_strip(heading: &str, accessory: Option<&gtk::Widget>) -> (gtk::Box, gtk::Box) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(NODE_GAP)
        .valign(gtk::Align::Start)
        .margin_start(ROW_INSET)
        .margin_end(ROW_INSET)
        .margin_top(ROW_INSET)
        .margin_bottom(ROW_INSET)
        .build();
    let wrap = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .halign(gtk::Align::Center)
        .margin_bottom(SCROLLBAR_ROOM)
        .build();
    let title = section_heading(heading);
    title.set_margin_start(ROW_INSET);
    title.set_margin_top(6);
    title.set_hexpand(true);
    title.set_valign(gtk::Align::BaselineCenter);
    let heading_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .baseline_position(gtk::BaselinePosition::Center)
        .margin_top(6)
        .build();
    heading_row.append(&title);
    if let Some(accessory) = accessory {
        accessory.set_valign(gtk::Align::BaselineCenter);
        heading_row.append(accessory);
    }
    wrap.append(&heading_row);
    wrap.append(&flow_arrow());
    wrap.append(&row);
    let scroller = gtk::ScrolledWindow::builder()
        .vscrollbar_policy(gtk::PolicyType::Never)
        .hexpand(true)
        .vexpand(false)
        .child(&wrap)
        .build();
    let strip = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    strip.append(&scroller);
    (strip, row)
}

pub(super) fn flow_arrow() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_height(12)
        .margin_start(ROW_INSET)
        .margin_end(ROW_INSET)
        .margin_top(8)
        .css_classes(["flow-arrow"])
        .build();
    area.set_draw_func(|area, cr, width, height| {
        let color = area.color();
        let (width, height) = (f64::from(width), f64::from(height));
        let middle = (height / 2.0).floor() + 0.5;
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()) * 0.35,
        );
        cr.set_line_width(1.0);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        cr.move_to(0.5, middle);
        cr.line_to(width - 1.0, middle);
        cr.move_to(width - 7.0, middle - 4.5);
        cr.line_to(width - 1.0, middle);
        cr.line_to(width - 7.0, middle + 4.5);
        if let Err(error) = cr.stroke() {
            tracing::debug!(%error, "drawing the signal flow arrow");
        }
    });
    area
}

pub(super) fn refresh_node_state(card: &Card, selected: bool) {
    if selected {
        card.root.add_css_class("selected");
    } else {
        card.root.remove_css_class("selected");
    }
}

pub(super) fn mini_node(spec: &'static BlockSpec) -> gtk::Box {
    let title = tr(spec.title);
    let icon = gtk::Image::builder()
        .icon_name(spec.icon)
        .pixel_size(MINI_ICON)
        .hexpand(true)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    let node = gtk::Box::builder()
        .css_classes(["chain-mini"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .tooltip_text(&title)
        .build();
    node.append(&icon);
    node.update_property(&[gtk::accessible::Property::Label(&title)]);
    node
}

pub(super) const NODE_WIDTH: i32 = 104;

pub(super) const NODE_HEIGHT: i32 = 96;

pub(super) const NODE_GAP: i32 = 20;

pub(super) const ROW_INSET: i32 = 12;

pub(super) const NODE_ICON: i32 = 28;

pub(super) const NODE_CHARS: i32 = 11;

pub(super) const NODE_INSET: i32 = 10;

pub(super) const POWER_INSET: i32 = 4;

pub(super) const MINI_ICON: i32 = 16;

impl Ui {
    pub(super) fn connect_chain_picker(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.chain_picker.connect_selected_notify(move |picker| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if ui.updating.get() {
                return;
            }
            let Ok(pattern) = u8::try_from(picker.selected()) else {
                return;
            };
            let current = ui.current_pattern();
            if current == Some(pattern) || !ui.chain_editable() {
                ui.refresh_chain_picker();
                return;
            }
            glib::idle_add_local_once(move || ui.write_chain(pattern, false));
        });
    }

    pub(super) fn current_pattern(&self) -> Option<u8> {
        let profile = self.profile()?;
        let model = self.model.borrow();
        profile.chain_pattern_of(model.image.as_ref()?)
    }

    pub(super) fn chain_write_queued(&self) -> bool {
        self.jobs
            .borrow()
            .iter()
            .any(|job| matches!(job, Job::WriteChain { .. }))
    }

    pub(super) fn on_chain_written(&self, job: Option<&Job>, outcome: ChainOutcome) {
        let undo = matches!(job, Some(Job::WriteChain { undo: true }));
        let normalised = outcome.is_normalised();
        let changed = outcome.packets > 0;
        let previous = outcome.previous;
        tracing::info!(
            requested = outcome.requested,
            actual = outcome.actual,
            order = %hex(&outcome.order),
            normalised,
            "chain pattern written"
        );
        self.adopt_image(outcome.image);
        if !changed {
            return;
        }
        let title = if normalised {
            tr("The amp kept a different signal chain")
        } else if undo {
            tr("Signal chain change undone")
        } else {
            tr("Signal chain changed")
        };
        self.window
            .upcast_ref::<gtk::Widget>()
            .announce(&title, gtk::AccessibleAnnouncementPriority::Medium);
        let mut builder = adw::Toast::builder().title(&title).use_markup(false);
        if !undo {
            builder = builder
                .button_label(tr("Undo"))
                .action_name("win.undo-chain")
                .action_target(&previous.to_variant());
        }
        let toast = builder.build();
        if let Some(old) = self.chain_toast.replace(Some(toast.clone())) {
            old.dismiss();
        }
        self.toasts.add_toast(toast);
    }

    pub(super) fn chain_editable(&self) -> bool {
        self.can_write()
            && self.jobs.borrow().iter().all(|job| {
                matches!(
                    job,
                    Job::WriteChain { .. } | Job::WriteParams | Job::ListPresets
                )
            })
    }

    pub(super) fn write_chain(self: &Rc<Self>, pattern: u8, undo: bool) {
        let Some(profile) = self.profile() else {
            return;
        };
        {
            let mut model = self.model.borrow_mut();
            let Some(image) = model.image.as_mut() else {
                return;
            };
            match profile.with_chain_pattern(image, pattern) {
                Ok(next) => *image = next,
                Err(error) => {
                    drop(model);
                    tracing::warn!(
                        error = &error as &dyn std::error::Error,
                        "refusing chain write"
                    );
                    self.toast(&report::chain(&error));
                    return;
                }
            }
        }
        tracing::info!(pattern, "changing the signal chain pattern");
        if let Some(old) = self.chain_toast.take() {
            old.dismiss();
        }
        self.flush_writes_now();
        self.send(Command::WriteChain(pattern), Job::WriteChain { undo });
        self.refresh_all();
    }

    pub(super) fn select_block(self: &Rc<Self>, key: &'static str) {
        if self.model.borrow().selected == key {
            return;
        }
        self.model.borrow_mut().selected = key;
        self.refresh_all();
    }

    pub(super) fn refresh_chain_picker(&self) {
        let previous = self.updating.replace(true);
        let selected = self
            .current_pattern()
            .map_or(gtk::INVALID_LIST_POSITION, u32::from);
        if self.chain_picker.selected() != selected {
            self.chain_picker.set_selected(selected);
        }
        self.updating.set(previous);
        self.chain_picker
            .set_sensitive(self.chain_editable() && self.current_pattern().is_some());
    }

    pub(super) fn refresh_chain(self: &Rc<Self>) {
        let model = self.model.borrow();
        let Some(image) = model.image.as_ref() else {
            return;
        };
        let order: Vec<&'static BlockSpec> = blocks::chain_order(self.table, image)
            .into_iter()
            .chain(blocks::extras())
            .collect();
        let keys: Vec<&'static str> = order.iter().map(|spec| spec.key).collect();
        if *self.chain_keys.borrow() != keys {
            self.rebuild_chain(&order);
            *self.chain_keys.borrow_mut() = keys;
        }
        let chain: Vec<&'static str> = order
            .iter()
            .filter(|spec| blocks::reorderable(spec))
            .map(|spec| spec.key)
            .collect();
        let count = chain.len().to_string();
        let can_edit = writable(&model);
        let previous = self.updating.replace(true);
        for card in self.cards.borrow().iter() {
            let Some(spec) = blocks::spec(card.key) else {
                continue;
            };
            let title = tr(spec.title);
            if let Some(position) = chain.iter().position(|key| *key == card.key) {
                let position = (position + 1).to_string();
                let text = trf(
                    "Position {position} of {count} in the signal chain. Choose the order with Signal Chain Order.",
                    &[("position", &position), ("count", &count)],
                );
                card.root
                    .update_property(&[gtk::accessible::Property::Description(&text)]);
            }
            refresh_node_state(card, card.key == model.selected);
            let on = blocks::is_on(self.table, image, spec);
            if on == Some(false) {
                card.root.add_css_class("block-off");
            } else {
                card.root.remove_css_class("block-off");
            }
            if let (Some(power), Some(on)) = (&card.power, on) {
                power.set_active(on);
                power.set_sensitive(can_edit);
                let tip = if on {
                    trf("Turn {block} Off", &[("block", &title)])
                } else {
                    trf("Turn {block} On", &[("block", &title)])
                };
                power.set_tooltip_text(Some(&tip));
            }
            let variant = blocks::variant(self.table, image, spec);
            for class in [
                "variant-green",
                "variant-red",
                "variant-yellow",
                "variant-none",
            ] {
                card.dot.remove_css_class(class);
            }
            card.dot
                .add_css_class(variant.map_or("variant-none", Variant::css_class));
            let variant_text =
                variant.map_or_else(|| tr("Unknown Variant"), |variant| tr(variant.title()));
            card.dot.set_tooltip_text(Some(&variant_text));
            card.dot
                .update_property(&[gtk::accessible::Property::Label(&variant_text)]);
            let type_text = blocks::type_label(self.table, image, spec).unwrap_or_default();
            card.type_label.set_text(&type_text);
            card.type_label.set_visible(!type_text.is_empty());
            let tooltip = if type_text.is_empty() {
                title.clone()
            } else {
                format!("{title}\n{type_text}")
            };
            card.root.set_tooltip_text(Some(&tooltip));
        }
        self.updating.set(previous);
        drop(model);
        self.refresh_chain_picker();
        self.fit_detail();
    }

    pub(super) fn rebuild_chain(self: &Rc<Self>, order: &[&'static BlockSpec]) {
        for container in [&self.chain_box, &self.extras_box] {
            while let Some(child) = container.first_child() {
                container.remove(&child);
            }
        }
        let mut cards = Vec::new();
        for spec in order {
            if spec.mini {
                self.chain_box.append(&mini_node(spec));
                continue;
            }
            let card = self.make_card(spec);
            if spec.chain_label.is_none() {
                self.extras_box.append(&card.root);
            } else {
                self.chain_box.append(&card.root);
            }
            cards.push(card);
        }
        *self.cards.borrow_mut() = cards;
        self.fit_detail();
    }

    pub(super) fn make_card(self: &Rc<Self>, spec: &'static BlockSpec) -> Card {
        let title = tr(spec.title);
        let icon = gtk::Image::builder()
            .icon_name(spec.icon)
            .pixel_size(NODE_ICON)
            .css_classes(["node-icon"])
            .build();
        let name = gtk::Label::builder()
            .label(&title)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(NODE_CHARS)
            .css_classes(["node-title"])
            .build();
        let type_label = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(NODE_CHARS)
            .css_classes(["caption", "dim-label", "block-type"])
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .valign(gtk::Align::Center)
            .margin_start(8)
            .margin_end(8)
            .build();
        content.append(&icon);
        content.append(&name);
        content.append(&type_label);
        let root = gtk::Overlay::builder()
            .child(&content)
            .width_request(NODE_WIDTH)
            .height_request(NODE_HEIGHT)
            .focusable(true)
            .accessible_role(gtk::AccessibleRole::Button)
            .css_classes(["card", "chain-node"])
            .overflow(gtk::Overflow::Hidden)
            .build();
        if let Some(class) = spec.category.css_class() {
            root.add_css_class(class);
        }
        root.set_cursor_from_name(Some("pointer"));
        root.update_property(&[gtk::accessible::Property::Label(&title)]);
        let dot = gtk::Box::builder()
            .css_classes(["node-variant", "variant-none"])
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .margin_top(NODE_INSET)
            .margin_start(NODE_INSET)
            .visible(spec.select_param.is_some())
            .build();
        root.add_overlay(&dot);
        let power = spec.on_param.map(|id| self.power_toggle(spec, id, &root));
        self.connect_node_select(spec, &root);
        Card {
            key: spec.key,
            root,
            dot,
            type_label,
            power,
        }
    }

    pub(super) fn power_toggle(
        self: &Rc<Self>,
        spec: &'static BlockSpec,
        id: &'static str,
        root: &gtk::Overlay,
    ) -> gtk::ToggleButton {
        let dot = gtk::Box::builder()
            .css_classes(["node-power-dot"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        let toggle = gtk::ToggleButton::builder()
            .child(&dot)
            .css_classes(["flat", "circular", "node-power"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(POWER_INSET)
            .margin_end(POWER_INSET)
            .build();
        toggle.update_property(&[gtk::accessible::Property::Label(&trf(
            "Enable {block}",
            &[("block", &tr(spec.title))],
        ))]);
        toggle.set_cursor_from_name(Some("pointer"));
        let weak = Rc::downgrade(self);
        toggle.connect_toggled(move |toggle| {
            if let Some(ui) = weak.upgrade()
                && !ui.updating.get()
            {
                ui.edit_param(id, Edit::Discrete(u16::from(toggle.is_active())));
            }
        });
        root.add_overlay(&toggle);
        toggle
    }

    pub(super) fn connect_node_select(
        self: &Rc<Self>,
        spec: &'static BlockSpec,
        root: &gtk::Overlay,
    ) {
        let click = gtk::GestureClick::builder()
            .button(gtk::gdk::BUTTON_PRIMARY)
            .build();
        click.connect_pressed(|gesture, _, _, _| {
            if let Some(widget) = gesture.widget() {
                widget.set_state_flags(gtk::StateFlags::ACTIVE, false);
            }
        });
        click.connect_end(|gesture, _| {
            if let Some(widget) = gesture.widget() {
                widget.unset_state_flags(gtk::StateFlags::ACTIVE);
            }
        });
        let click_ui = Rc::downgrade(self);
        let click_root = root.downgrade();
        click.connect_released(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let Some(root) = click_root.upgrade() {
                root.grab_focus();
            }
            if let Some(ui) = click_ui.upgrade() {
                ui.select_block(spec.key);
            }
        });
        root.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let key_ui = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, pressed, _, state| {
            let plain = !state.intersects(
                gtk::gdk::ModifierType::ALT_MASK | gtk::gdk::ModifierType::CONTROL_MASK,
            );
            let activate = matches!(
                pressed,
                gtk::gdk::Key::Return
                    | gtk::gdk::Key::KP_Enter
                    | gtk::gdk::Key::space
                    | gtk::gdk::Key::KP_Space
            );
            match key_ui.upgrade() {
                Some(ui) if plain && activate => {
                    ui.select_block(spec.key);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        root.add_controller(keys);
    }
}
