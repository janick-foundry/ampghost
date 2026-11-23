use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use ampghost_engine::{Command, ParamWrite};
use gtk::glib;

use crate::blocks;
use crate::i18n::tr;
use crate::jobs::Job;
use crate::params_view::{self, Edit, OnChange, Structure};

use super::Ui;
use super::chain::ROW_INSET;
use super::model::writable;

pub(super) const CONTINUOUS_WRITE_DEBOUNCE: Duration = Duration::from_millis(500);

pub(super) fn build_editor(
    channel_section: &adw::Clamp,
    chain: &gtk::Box,
    detail: &adw::Clamp,
    output: &gtk::Box,
) -> gtk::Box {
    let editor = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    editor.append(channel_section);
    editor.append(chain);
    editor.append(detail);
    editor.append(output);
    editor
}

pub(super) struct Detail {
    pub(super) clamp: adw::Clamp,
    pub(super) holder: adw::Bin,
    pub(super) icon: gtk::Image,
    pub(super) title: gtk::Label,
    pub(super) subtitle: gtk::Label,
}

pub(super) fn build_detail() -> Detail {
    let icon = gtk::Image::builder()
        .pixel_size(DETAIL_ICON)
        .valign(gtk::Align::Center)
        .css_classes(["detail-icon"])
        .build();
    let title = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["title-2"])
        .build();
    let subtitle = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["dim-label"])
        .build();
    let titles = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    titles.append(&title);
    titles.append(&subtitle);
    let head = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["detail-head"])
        .build();
    head.append(&icon);
    head.append(&titles);
    let holder = adw::Bin::builder().vexpand(true).build();
    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .css_classes(["card", "detail-sheet"])
        .overflow(gtk::Overflow::Hidden)
        .build();
    panel.append(&head);
    panel.append(&holder);
    let clamp = adw::Clamp::builder()
        .child(&panel)
        .vexpand(true)
        .margin_start(DETAIL_MARGIN)
        .margin_end(DETAIL_MARGIN)
        .build();
    Detail {
        clamp,
        holder,
        icon,
        title,
        subtitle,
    }
}

pub(super) const DETAIL_ICON: i32 = 32;

pub(super) const DETAIL_MARGIN: i32 = 12;

pub(super) const SCROLLBAR_ROOM: i32 = 16;

pub(super) fn section_heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .margin_start(16)
        .margin_top(12)
        .css_classes(["heading"])
        .build()
}

impl Ui {
    pub(super) fn edit_param(self: &Rc<Self>, id: &'static str, edit: Edit) {
        if self.updating.get() || !self.can_write() {
            return;
        }
        match edit {
            Edit::Discrete(value) => {
                if self.stage_write(id, value) {
                    self.cancel_write_timer(id);
                    self.flush_writes();
                }
            }
            Edit::Continuous(value) => {
                if self.stage_write(id, value) {
                    self.schedule_write(id);
                }
            }
            Edit::Released => {
                if self.pending.borrow().contains_key(id) {
                    self.cancel_write_timer(id);
                    self.flush_writes();
                }
            }
        }
    }

    pub(super) fn stage_write(&self, id: &'static str, value: u16) -> bool {
        if self.pending.borrow().get(id) == Some(&value) {
            return false;
        }
        let written = self.in_flight.borrow().get(id).copied().or_else(|| {
            self.model
                .borrow()
                .image
                .as_ref()
                .and_then(|image| blocks::value(self.table, image, id))
        });
        if written == Some(value) {
            if self.pending.borrow_mut().remove(id).is_some() {
                self.cancel_write_timer(id);
                self.refresh_header();
            }
            return false;
        }
        self.pending.borrow_mut().insert(id, value);
        self.refresh_header();
        true
    }

    pub(super) fn schedule_write(self: &Rc<Self>, id: &'static str) {
        self.cancel_write_timer(id);
        let ui = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(CONTINUOUS_WRITE_DEBOUNCE, move || {
            if let Some(ui) = ui.upgrade() {
                ui.write_timers.borrow_mut().remove(id);
                ui.flush_writes();
            }
        });
        self.write_timers.borrow_mut().insert(id, source);
    }

    pub(super) fn cancel_write_timer(&self, id: &str) {
        if let Some(source) = self.write_timers.borrow_mut().remove(id) {
            source.remove();
        }
    }

    pub(super) fn cancel_write_timers(&self) {
        let timers = std::mem::take(&mut *self.write_timers.borrow_mut());
        for source in timers.into_values() {
            source.remove();
        }
    }

    pub(super) fn drop_pending_writes(&self, reason: &str) {
        self.cancel_write_timers();
        let dropped = std::mem::take(&mut *self.pending.borrow_mut());
        if !dropped.is_empty() {
            tracing::info!(?dropped, reason, "dropping unsent parameter changes");
        }
    }

    pub(super) fn flush_writes_now(&self) {
        self.cancel_write_timers();
        if !self.can_write() {
            return;
        }
        let writes = self.take_pending_writes();
        if writes.is_empty() {
            return;
        }
        tracing::info!(?writes, "writing parameters before the next step");
        self.send(Command::WriteParams(writes), Job::WriteParams);
    }

    pub(super) fn take_pending_writes(&self) -> Vec<ParamWrite> {
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        self.in_flight.borrow_mut().extend(pending.iter());
        pending
            .into_iter()
            .map(|(id, value)| ParamWrite {
                id: id.to_owned(),
                value,
            })
            .collect()
    }

    pub(super) fn flush_writes(&self) {
        if self.is_busy() || !self.can_write() {
            return;
        }
        let writes = self.take_pending_writes();
        if writes.is_empty() {
            return;
        }
        tracing::info!(?writes, "writing parameters");
        self.send(Command::WriteParams(writes), Job::WriteParams);
    }

    pub(super) fn fit_detail(&self) {
        let (_, natural, _, _) = self.chain_box.measure(gtk::Orientation::Horizontal, -1);
        let width = (natural - 2 * ROW_INSET).max(0);
        if width > 0 && self.detail_clamp.maximum_size() != width {
            self.detail_clamp.set_maximum_size(width);
            self.detail_clamp.set_tightening_threshold(width);
        }
    }

    pub(super) fn refresh_params(self: &Rc<Self>) {
        let model = self.model.borrow();
        let Some(image) = model.image.as_ref() else {
            return;
        };
        let Some(spec) = blocks::spec(model.selected) else {
            return;
        };
        self.detail_title.set_text(&tr(spec.title));
        self.detail_icon.set_icon_name(Some(spec.icon));
        let type_text = blocks::type_label(self.table, image, spec).unwrap_or_default();
        self.detail_subtitle.set_text(&type_text);
        self.detail_subtitle.set_visible(!type_text.is_empty());
        let structure = Structure {
            block: spec.key,
            type_value: blocks::type_value(self.table, image, spec),
            writable: writable(&model),
        };
        let current = self
            .params
            .borrow()
            .as_ref()
            .map(|view| view.structure.clone());
        if current.as_ref() == Some(&structure) {
            let pending = self.pending.borrow();
            if let Some(view) = self.params.borrow().as_ref() {
                view.update(self.table, image, &self.updating, &|id| {
                    pending.contains_key(id)
                });
            }
            return;
        }
        let weak = Rc::downgrade(self);
        let on_change: OnChange = Rc::new(move |id, edit| {
            if let Some(ui) = weak.upgrade() {
                ui.edit_param(id, edit);
            }
        });
        let view = params_view::build(
            self.table,
            image,
            spec,
            structure,
            &self.updating,
            &on_change,
        );
        self.params_holder.set_child(Some(&view.root));
        *self.params.borrow_mut() = Some(view);
    }
}
