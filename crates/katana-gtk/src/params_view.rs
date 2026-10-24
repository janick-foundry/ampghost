use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use katana_params::{EnumOption, Param, ParamKind, ParamTable};
use katana_protocol::PatchImage;

use crate::blocks::{self, BlockSpec, Variant};
use crate::i18n::{tr, trf};

const SPIN_THRESHOLD: u16 = 127;
pub const CONTENT_WIDTH: i32 = 680;
pub const CONTENT_TIGHTENING: i32 = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Discrete(u16),
    Continuous(u16),
    Released,
}

pub type OnChange = Rc<dyn Fn(&'static str, Edit)>;

type Updater = Box<dyn Fn(u16)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Structure {
    pub block: &'static str,
    pub type_value: Option<u16>,
    pub writable: bool,
}

pub struct ParamsView {
    pub root: gtk::ScrolledWindow,
    pub structure: Structure,
    updaters: Vec<(&'static str, Updater)>,
}

impl ParamsView {
    pub fn update(
        &self,
        table: &ParamTable,
        image: &PatchImage,
        guard: &Cell<bool>,
        skip: &dyn Fn(&str) -> bool,
    ) {
        let previous = guard.replace(true);
        for (id, update) in &self.updaters {
            if skip(id) {
                continue;
            }
            if let Some(value) = blocks::value(table, image, id) {
                update(value);
            }
        }
        guard.set(previous);
    }
}

pub fn build(
    table: &'static ParamTable,
    image: &PatchImage,
    spec: &'static BlockSpec,
    structure: Structure,
    guard: &Rc<Cell<bool>>,
    on_change: &OnChange,
) -> ParamsView {
    let previous = guard.replace(true);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(12)
        .margin_end(12)
        .build();
    let clamp = adw::Clamp::builder()
        .maximum_size(CONTENT_WIDTH)
        .tightening_threshold(CONTENT_TIGHTENING)
        .child(&content)
        .build();
    let root = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();
    let mut updaters = Vec::new();
    let main = Section::new(
        None,
        (!structure.writable).then(|| tr("Writing is disabled.")),
    );
    let leading = [spec.on_param, spec.select_param, spec.type_param];
    for id in leading.into_iter().flatten() {
        let Ok(param) = table.get(id) else {
            continue;
        };
        let title = match Some(id) {
            id if id == spec.on_param => tr("Enabled"),
            id if id == spec.select_param => tr("Variant"),
            _ => tr("Type"),
        };
        let (row, update) = match param.kind() {
            ParamKind::Enum { options } if Some(id) == spec.select_param => {
                variant_row(param, options, &title, guard, on_change)
            }
            _ => make_row(param, &title, guard, on_change),
        };
        row.set_sensitive(structure.writable);
        main.add(&row);
        updaters.push((param.id(), update));
    }
    content.append(&main.root);
    let params = blocks::block_params(table, image, spec);
    let details = Section::new(
        Some(tr("Parameters")),
        blocks::type_label(table, image, spec)
            .map(|label| trf("Settings for {type}", &[("type", &label)])),
    );
    if params.is_empty() {
        details.add(
            &adw::ActionRow::builder()
                .title(tr("No further parameters"))
                .title_lines(1)
                .build(),
        );
    }
    for param in params {
        let label = blocks::pretty_label(param.label());
        let (row, update) = make_row(param, &label, guard, on_change);
        row.set_sensitive(structure.writable);
        details.add(&row);
        updaters.push((param.id(), update));
    }
    content.append(&details.root);
    let view = ParamsView {
        root,
        structure,
        updaters,
    };
    view.update(table, image, guard, &|_| false);
    guard.set(previous);
    view
}

struct Section {
    root: gtk::Box,
    list: gtk::ListBox,
}

impl Section {
    fn new(title: Option<String>, description: Option<String>) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        if let Some(title) = title {
            root.append(
                &gtk::Label::builder()
                    .label(title)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["heading", "detail-section-title"])
                    .build(),
            );
        }
        if let Some(description) = description {
            root.append(
                &gtk::Label::builder()
                    .label(description)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["dim-label", "caption", "detail-section-title"])
                    .build(),
            );
        }
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["detail-list"])
            .build();
        root.append(&list);
        Self { root, list }
    }

    fn add(&self, row: &impl IsA<gtk::Widget>) {
        self.list.append(row);
    }
}

fn make_row(
    param: &'static Param,
    title: &str,
    guard: &Rc<Cell<bool>>,
    on_change: &OnChange,
) -> (gtk::Widget, Updater) {
    match param.kind() {
        ParamKind::Enum { options } if is_switch(options) => {
            switch_row(param, title, guard, on_change)
        }
        ParamKind::Enum { options } => combo_row(param, options, title, guard, on_change),
        ParamKind::Range {
            raw_min, raw_max, ..
        }
        | ParamKind::Table {
            raw_min, raw_max, ..
        } => {
            if raw_max.saturating_sub(*raw_min) > SPIN_THRESHOLD {
                spin_row(param, *raw_min, *raw_max, title, guard, on_change)
            } else {
                scale_row(param, *raw_min, *raw_max, title, guard, on_change)
            }
        }
    }
}

fn is_switch(options: &[EnumOption]) -> bool {
    matches!(options, [off, on]
        if off.value == 0 && on.value == 1
            && off.label.eq_ignore_ascii_case("off")
            && on.label.eq_ignore_ascii_case("on"))
}

fn switch_row(
    param: &'static Param,
    title: &str,
    guard: &Rc<Cell<bool>>,
    on_change: &OnChange,
) -> (gtk::Widget, Updater) {
    let row = adw::SwitchRow::builder()
        .title(title)
        .title_lines(1)
        .use_markup(false)
        .build();
    let id = param.id();
    let handler_guard = Rc::clone(guard);
    let callback = Rc::clone(on_change);
    row.connect_active_notify(move |row| {
        if !handler_guard.get() {
            callback(id, Edit::Discrete(u16::from(row.is_active())));
        }
    });
    let target = row.clone();
    (
        row.upcast(),
        Box::new(move |value| target.set_active(value != 0)),
    )
}

fn combo_row(
    param: &'static Param,
    options: &'static [EnumOption],
    title: &str,
    guard: &Rc<Cell<bool>>,
    on_change: &OnChange,
) -> (gtk::Widget, Updater) {
    let labels: Vec<String> = options
        .iter()
        .map(|option| blocks::pretty_label(&option.label))
        .collect();
    let model = gtk::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
    let row = adw::ComboRow::builder()
        .title(title)
        .title_lines(1)
        .use_markup(false)
        .model(&model)
        .build();
    if options.len() > 12 {
        row.set_expression(Some(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            None::<gtk::Expression>,
            "string",
        )));
        row.set_enable_search(true);
    }
    let id = param.id();
    let handler_guard = Rc::clone(guard);
    let callback = Rc::clone(on_change);
    row.connect_selected_notify(move |row| {
        if handler_guard.get() {
            return;
        }
        let chosen = usize::try_from(row.selected())
            .ok()
            .and_then(|index| options.get(index));
        if let Some(option) = chosen {
            callback(id, Edit::Discrete(option.value));
        }
    });
    let target = row.clone();
    (
        row.upcast(),
        Box::new(move |value| {
            let position = options
                .iter()
                .position(|option| option.value == value)
                .and_then(|index| u32::try_from(index).ok())
                .unwrap_or(gtk::INVALID_LIST_POSITION);
            target.set_selected(position);
        }),
    )
}

fn variant_row(
    param: &'static Param,
    options: &'static [EnumOption],
    title: &str,
    guard: &Rc<Cell<bool>>,
    on_change: &OnChange,
) -> (gtk::Widget, Updater) {
    let variants: Vec<(&'static EnumOption, Variant)> = options
        .iter()
        .filter_map(|option| Variant::from_label(&option.label).map(|variant| (option, variant)))
        .collect();
    if variants.len() != options.len() {
        return combo_row(param, options, title, guard, on_change);
    }
    let group = adw::ToggleGroup::builder()
        .valign(gtk::Align::Center)
        .homogeneous(true)
        .css_classes(["variant-selector"])
        .build();
    group
        .upcast_ref::<gtk::Widget>()
        .update_property(&[gtk::accessible::Property::Label(title)]);
    let values: Vec<u16> = variants.iter().map(|(option, _)| option.value).collect();
    for (option, variant) in variants {
        let label = blocks::pretty_label(&option.label);
        let dot = gtk::Box::builder()
            .css_classes(["variant-dot", variant.css_class()])
            .valign(gtk::Align::Center)
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::Center)
            .build();
        content.append(&dot);
        content.append(&gtk::Label::new(Some(&label)));
        group.add(
            adw::Toggle::builder()
                .child(&content)
                .label(label.as_str())
                .description(label.as_str())
                .tooltip(tr(variant.title()))
                .build(),
        );
    }
    let id = param.id();
    let handler_guard = Rc::clone(guard);
    let callback = Rc::clone(on_change);
    let handler_values = values.clone();
    group.connect_active_notify(move |group| {
        if handler_guard.get() {
            return;
        }
        let chosen = usize::try_from(group.active())
            .ok()
            .and_then(|index| handler_values.get(index));
        if let Some(&value) = chosen {
            callback(id, Edit::Discrete(value));
        }
    });
    let row = adw::ActionRow::builder()
        .title(title)
        .title_lines(1)
        .use_markup(false)
        .build();
    row.add_suffix(&group);
    (
        row.upcast(),
        Box::new(move |value| {
            let position = values
                .iter()
                .position(|&option| option == value)
                .and_then(|index| u32::try_from(index).ok())
                .unwrap_or(gtk::INVALID_LIST_POSITION);
            group.set_active(position);
        }),
    )
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_raw(value: f64) -> u16 {
    value.round().clamp(0.0, f64::from(u16::MAX)) as u16
}

fn ignore_wheel(widget: &gtk::Widget) {
    if widget.is::<gtk::ScrolledWindow>() {
        return;
    }
    let controllers = widget.observe_controllers();
    let wheels: Vec<gtk::EventControllerScroll> = (0..controllers.n_items())
        .filter_map(|index| controllers.item(index))
        .filter_map(|item| item.downcast::<gtk::EventControllerScroll>().ok())
        .collect();
    for wheel in wheels {
        widget.remove_controller(&wheel);
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        ignore_wheel(&current);
        child = current.next_sibling();
    }
}

fn slider_row(title: &str, scale: &gtk::Scale, value_label: &gtk::Label) -> gtk::ListBoxRow {
    let title_label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let top = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .build();
    top.append(&title_label);
    top.append(value_label);
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .css_classes(["detail-slider"])
        .build();
    body.append(&top);
    body.append(scale);
    gtk::ListBoxRow::builder()
        .activatable(false)
        .child(&body)
        .build()
}

fn scale_row(
    param: &'static Param,
    raw_min: u16,
    raw_max: u16,
    title: &str,
    guard: &Rc<Cell<bool>>,
    on_change: &OnChange,
) -> (gtk::Widget, Updater) {
    let adjustment = gtk::Adjustment::new(
        f64::from(raw_min),
        f64::from(raw_min),
        f64::from(raw_max),
        1.0,
        5.0,
        0.0,
    );
    let scale = gtk::Scale::builder()
        .orientation(gtk::Orientation::Horizontal)
        .adjustment(&adjustment)
        .draw_value(false)
        .digits(0)
        .round_digits(0)
        .hexpand(true)
        .css_classes(["detail-scale"])
        .build();
    scale.update_property(&[gtk::accessible::Property::Label(title)]);
    ignore_wheel(scale.upcast_ref());
    let value_label = gtk::Label::builder()
        .xalign(1.0)
        .css_classes(["numeric", "dim-label"])
        .build();
    let row = slider_row(title, &scale, &value_label);
    let id = param.id();
    let handler_guard = Rc::clone(guard);
    let callback = Rc::clone(on_change);
    let handler_label = value_label.clone();
    let dragging = Rc::new(Cell::new(false));
    let deferred: Rc<Cell<Option<u16>>> = Rc::new(Cell::new(None));
    let pointer = gtk::EventControllerLegacy::builder()
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    let pointer_guard = Rc::clone(guard);
    let pointer_callback = Rc::clone(on_change);
    let pointer_dragging = Rc::clone(&dragging);
    let pointer_deferred = Rc::clone(&deferred);
    let pointer_adjustment = adjustment.clone();
    let pointer_label = value_label.clone();
    pointer.connect_event(move |_, event| {
        match event.event_type() {
            gtk::gdk::EventType::ButtonPress | gtk::gdk::EventType::TouchBegin => {
                pointer_dragging.set(true);
            }
            gtk::gdk::EventType::ButtonRelease
            | gtk::gdk::EventType::TouchEnd
            | gtk::gdk::EventType::TouchCancel
                if pointer_dragging.replace(false) =>
            {
                if !pointer_guard.get() {
                    pointer_callback(id, Edit::Released);
                }
                if let Some(value) = pointer_deferred.take()
                    && to_raw(pointer_adjustment.value()) == value
                {
                    let previous = pointer_guard.replace(true);
                    pointer_adjustment.set_value(f64::from(value));
                    pointer_label.set_text(&param.describe(value));
                    pointer_guard.set(previous);
                }
            }
            _ => {}
        }
        gtk::glib::Propagation::Proceed
    });
    scale.add_controller(pointer);
    scale.connect_value_changed(move |scale| {
        let raw = to_raw(scale.value());
        handler_label.set_text(&param.describe(raw));
        if !handler_guard.get() {
            callback(id, Edit::Continuous(raw));
        }
    });
    (
        row.upcast(),
        Box::new(move |value| {
            if dragging.get() {
                deferred.set(Some(value));
                return;
            }
            deferred.set(None);
            adjustment.set_value(f64::from(value));
            value_label.set_text(&param.describe(value));
        }),
    )
}

fn spin_row(
    param: &'static Param,
    raw_min: u16,
    raw_max: u16,
    title: &str,
    guard: &Rc<Cell<bool>>,
    on_change: &OnChange,
) -> (gtk::Widget, Updater) {
    let row = adw::SpinRow::with_range(f64::from(raw_min), f64::from(raw_max), 1.0);
    ignore_wheel(row.upcast_ref());
    row.set_title(title);
    row.set_title_lines(1);
    row.set_subtitle_lines(1);
    row.set_use_markup(false);
    let id = param.id();
    let handler_guard = Rc::clone(guard);
    let callback = Rc::clone(on_change);
    row.connect_value_notify(move |row| {
        let raw = to_raw(row.value());
        row.set_subtitle(&param.describe(raw));
        if !handler_guard.get() {
            callback(id, Edit::Continuous(raw));
        }
    });
    let target = row.clone();
    (
        row.upcast(),
        Box::new(move |value| {
            target.set_value(f64::from(value));
            target.set_subtitle(&param.describe(value));
        }),
    )
}
