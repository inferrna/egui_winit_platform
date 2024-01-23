//! A platform integration to use [egui](https://github.com/emilk/egui) with [winit](https://github.com/rust-windowing/winit).
//!
//! You need to create a [`Platform`] and feed it with `winit::event::Event` events.
//! Use `begin_frame()` and `end_frame()` to start drawing the egui UI.
//! A basic usage example can be found [here](https://github.com/hasenbanck/egui_example).
#![warn(missing_docs)]

use std::collections::HashMap;

#[cfg(feature = "clipboard")]
use copypasta::{ClipboardContext, ClipboardProvider};
use egui::{
    emath::{pos2, vec2},
    Context, Key, Pos2,
};
use winit::{
    dpi::PhysicalSize,
    event::{Event, TouchPhase, WindowEvent::*},
    window::CursorIcon,
};
use winit::event::{KeyEvent, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, ModifiersState, NamedKey, NativeKeyCode, PhysicalKey};

/// Configures the creation of the `Platform`.
#[derive(Debug, Default)]
pub struct PlatformDescriptor {
    /// Width of the window in physical pixel.
    pub physical_width: u32,
    /// Height of the window in physical pixel.
    pub physical_height: u32,
    /// HiDPI scale factor.
    pub scale_factor: f64,
    /// Egui font configuration.
    pub font_definitions: egui::FontDefinitions,
    /// Egui style configuration.
    pub style: egui::Style,
}

#[cfg(feature = "webbrowser")]
fn handle_links(output: &egui::PlatformOutput) {
    if let Some(open_url) = &output.open_url {
        // This does not handle open_url.new_tab
        // webbrowser does not support web anyway
        if let Err(err) = webbrowser::open(&open_url.url) {
            eprintln!("Failed to open url: {}", err);
        }
    }
}

#[cfg(feature = "clipboard")]
fn handle_clipboard(output: &egui::PlatformOutput, clipboard: Option<&mut ClipboardContext>) {
    if !output.copied_text.is_empty() {
        if let Some(clipboard) = clipboard {
            if let Err(err) = clipboard.set_contents(output.copied_text.clone()) {
                eprintln!("Copy/Cut error: {}", err);
            }
        }
    }
}

/// Provides the integration between egui and winit.
pub struct Platform {
    scale_factor: f64,
    context: Context,
    raw_input: egui::RawInput,
    modifier_state: ModifiersState,
    pointer_pos: Option<egui::Pos2>,

    #[cfg(feature = "clipboard")]
    clipboard: Option<ClipboardContext>,

    // For emulating pointer events from touch events we merge multi-touch
    // pointers, and ref-count the press state.
    touch_pointer_pressed: u32,

    // Egui requires unique u64 device IDs for touch events but Winit's
    // device IDs are opaque, so we have to create our own ID mapping.
    device_indices: HashMap<winit::event::DeviceId, u64>,
    next_device_index: u64,
}

impl Platform {
    /// Creates a new `Platform`.
    pub fn new(descriptor: PlatformDescriptor) -> Self {
        let context = Context::default();

        context.set_fonts(descriptor.font_definitions.clone());
        context.set_style(descriptor.style);
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::default(),
                vec2(
                    descriptor.physical_width as f32,
                    descriptor.physical_height as f32,
                ) / descriptor.scale_factor as f32,
            )),
            ..Default::default()
        };

        Self {
            scale_factor: descriptor.scale_factor,
            context,
            raw_input,
            modifier_state: ModifiersState::empty(),
            pointer_pos: Some(Pos2::default()),
            #[cfg(feature = "clipboard")]
            clipboard: ClipboardContext::new().ok(),
            touch_pointer_pressed: 0,
            device_indices: HashMap::new(),
            next_device_index: 1,
        }
    }

    /// Handles the given winit event and updates the egui context. Should be called before starting a new frame with `start_frame()`.
    pub fn handle_event<T>(&mut self, winit_event: &Event<T>) {
        match winit_event {
            Event::WindowEvent {
                window_id: _window_id,
                event,
            } => match event {
                // Resize with 0 width and height is used by winit to signal a minimize event on Windows.
                // See: https://github.com/rust-windowing/winit/issues/208
                // There is nothing to do for minimize events, so it is ignored here. This solves an issue where
                // egui window positions would be changed when minimizing on Windows.
                Resized(PhysicalSize {
                    width: 0,
                    height: 0,
                }) => {}
                Resized(physical_size) => {
                    self.raw_input.screen_rect = Some(egui::Rect::from_min_size(
                        Default::default(),
                        vec2(physical_size.width as f32, physical_size.height as f32)
                            / self.scale_factor as f32,
                    ));
                }
                ScaleFactorChanged {
                    scale_factor,
                    inner_size_writer,
                } => {
                    let maybe_old_rect = self.raw_input.screen_rect.clone();
                    if let Some(old_rect) = maybe_old_rect {
                        let new_sz = old_rect.size() * (scale_factor / self.scale_factor) as f32;
                        self.raw_input.screen_rect = Some(egui::Rect::from_min_size(
                            Default::default(),
                            vec2(new_sz.x, new_sz.y),
                        ));
                    }
                    self.scale_factor = *scale_factor;
                    self.context.set_pixels_per_point(*scale_factor as f32);
                }
                MouseInput { state, button, .. } => {
                    if let winit::event::MouseButton::Other(..) = button {
                    } else {
                        // push event only if the cursor is inside the window
                        if let Some(pointer_pos) = self.pointer_pos {
                            self.raw_input.events.push(egui::Event::PointerButton {
                                pos: pointer_pos,
                                button: match button {
                                    winit::event::MouseButton::Left => egui::PointerButton::Primary,
                                    winit::event::MouseButton::Right => {
                                        egui::PointerButton::Secondary
                                    }
                                    winit::event::MouseButton::Middle => {
                                        egui::PointerButton::Middle
                                    }
                                    winit::event::MouseButton::Other(_) => unreachable!(),
                                    MouseButton::Back => {
                                        egui::PointerButton::Extra1
                                    }
                                    MouseButton::Forward => {
                                        egui::PointerButton::Extra2
                                    }
                                },
                                pressed: *state == winit::event::ElementState::Pressed,
                                modifiers: Default::default(),
                            });
                        }
                    }
                }
                Touch(touch) => {
                    let pointer_pos = pos2(
                        touch.location.x as f32 / self.scale_factor as f32,
                        touch.location.y as f32 / self.scale_factor as f32,
                    );

                    let device_id = match self.device_indices.get(&touch.device_id) {
                        Some(id) => *id,
                        None => {
                            let device_id = self.next_device_index;
                            self.device_indices.insert(touch.device_id, device_id);
                            self.next_device_index += 1;
                            device_id
                        }
                    };
                    let egui_phase = match touch.phase {
                        TouchPhase::Started => egui::TouchPhase::Start,
                        TouchPhase::Moved => egui::TouchPhase::Move,
                        TouchPhase::Ended => egui::TouchPhase::End,
                        TouchPhase::Cancelled => egui::TouchPhase::Cancel,
                    };

                    let force = match touch.force {
                        Some(winit::event::Force::Calibrated { force, .. }) => force as f32,
                        Some(winit::event::Force::Normalized(force)) => force as f32,
                        None => 0.0f32, // hmmm, egui can't differentiate unsupported from zero pressure
                    };

                    self.raw_input.events.push(egui::Event::Touch {
                        device_id: egui::TouchDeviceId(device_id),
                        id: egui::TouchId(touch.id),
                        phase: egui_phase,
                        pos: pointer_pos,
                        force: Some(force),
                    });

                    // Currently Winit doesn't emulate pointer events based on
                    // touch events but Egui requires pointer emulation.
                    //
                    // For simplicity we just merge all touch pointers into a
                    // single virtual pointer and ref-count the press state
                    // (i.e. the pointer will remain pressed during multi-touch
                    // events until the last pointer is lifted up)

                    let was_pressed = self.touch_pointer_pressed > 0;

                    match touch.phase {
                        TouchPhase::Started => {
                            self.touch_pointer_pressed += 1;
                        }
                        TouchPhase::Ended | TouchPhase::Cancelled => {
                            self.touch_pointer_pressed = match self
                                .touch_pointer_pressed
                                .checked_sub(1)
                            {
                                Some(count) => count,
                                None => {
                                    eprintln!("Pointer emulation error: Unbalanced touch start/stop events from Winit");
                                    0
                                }
                            };
                        }
                        TouchPhase::Moved => {
                            self.raw_input
                                .events
                                .push(egui::Event::PointerMoved(pointer_pos));
                        }
                    }

                    if !was_pressed && self.touch_pointer_pressed > 0 {
                        self.raw_input.events.push(egui::Event::PointerButton {
                            pos: pointer_pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: Default::default(),
                        });
                    } else if was_pressed && self.touch_pointer_pressed == 0 {
                        // Egui docs say that the pressed=false should be sent _before_
                        // the PointerGone.
                        self.raw_input.events.push(egui::Event::PointerButton {
                            pos: pointer_pos,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: Default::default(),
                        });
                        self.raw_input.events.push(egui::Event::PointerGone);
                    }
                }
                MouseWheel { delta, .. } => {
                    let mut delta = match delta {
                        winit::event::MouseScrollDelta::LineDelta(x, y) => {
                            let line_height = 8.0; // TODO as in egui_glium
                            vec2(*x, *y) * line_height
                        }
                        winit::event::MouseScrollDelta::PixelDelta(delta) => {
                            vec2(delta.x as f32, delta.y as f32)
                        }
                    };
                    if cfg!(target_os = "macos") {
                        // See https://github.com/rust-windowing/winit/issues/1695 for more info.
                        delta.x *= -1.0;
                    }

                    // The ctrl (cmd on macos) key indicates a zoom is desired.
                    if self.raw_input.modifiers.ctrl || self.raw_input.modifiers.command {
                        self.raw_input
                            .events
                            .push(egui::Event::Zoom((delta.y / 200.0).exp()));
                    } else {
                        self.raw_input.events.push(egui::Event::Scroll(delta));
                    }
                }
                CursorMoved { position, .. } => {
                    let pointer_pos = pos2(
                        position.x as f32 / self.scale_factor as f32,
                        position.y as f32 / self.scale_factor as f32,
                    );
                    self.pointer_pos = Some(pointer_pos);
                    self.raw_input
                        .events
                        .push(egui::Event::PointerMoved(pointer_pos));
                }
                CursorLeft { .. } => {
                    self.pointer_pos = None;
                    self.raw_input.events.push(egui::Event::PointerGone);
                }
                ModifiersChanged(input) => {
                    self.modifier_state = input.state();
                    self.raw_input.modifiers = winit_to_egui_modifiers(input.state());
                }
                KeyboardInput { device_id, event, is_synthetic } => 'block: {
                    let virtual_keycode = event.logical_key.clone();
                    let virtual_keycode2 = event.logical_key.clone();
                    let pressed = event.state == winit::event::ElementState::Pressed;
                    let ctrl = self.modifier_state.control_key();

                    if let Some(txt) = event.clone().text {
                        if self.modifier_state.is_empty()
                        {
                            self.raw_input
                                .events
                                .push(egui::Event::Text(txt.to_string()));
                        }
                        break 'block;
                    }

                    match (pressed, ctrl, virtual_keycode) {
                        (true, true, winit::keyboard::Key::Named(NamedKey::Copy)) => {
                            self.raw_input.events.push(egui::Event::Copy)
                        }
                        (true, true, winit::keyboard::Key::Named(NamedKey::Cut)) => {
                            self.raw_input.events.push(egui::Event::Cut)
                        }
                        (true, true, winit::keyboard::Key::Named(NamedKey::Paste)) => {
                            #[cfg(feature = "clipboard")]
                            if let Some(ref mut clipboard) = self.clipboard {
                                if let Ok(contents) = clipboard.get_contents() {
                                    self.raw_input.events.push(egui::Event::Text(contents))
                                }
                            }
                        }
                        _ => {
                            if let Some(key) = winit_to_egui_key_code(virtual_keycode2) {
                                let physical_key = event.physical_key;
                                self.raw_input.events.push(egui::Event::Key {
                                    key,
                                    physical_key: winit_to_egui_physical_key_code(physical_key),
                                    pressed,
                                    repeat: false,
                                    modifiers: winit_to_egui_modifiers(self.modifier_state),
                                });
                            }
                        }
                    }
                }

                ActivationTokenDone { .. } => {}
                Moved(_) => {}
                CloseRequested => {}
                Destroyed => {}
                DroppedFile(_) => {}
                HoveredFile(_) => {}
                HoveredFileCancelled => {}
                Focused(_) => {}
                Ime(_) => {}
                CursorEntered { .. } => {}
                TouchpadMagnify { .. } => {}
                SmartMagnify { .. } => {}
                TouchpadRotate { .. } => {}
                TouchpadPressure { .. } => {}
                AxisMotion { .. } => {}
                ThemeChanged(_) => {}
                Occluded(_) => {}
                RedrawRequested => {}
            },
            Event::DeviceEvent { .. } => {}
            _ => {}
        }
    }

    /// Returns `true` if egui should handle the event exclusively. Check this to
    /// avoid unexpected interactions, e.g. a mouse click registering "behind" the UI.
    pub fn captures_event<T>(&self, winit_event: &Event<T>) -> bool {
        match winit_event {
            Event::WindowEvent {
                window_id: _window_id,
                event,
            } => match event {
                KeyboardInput { .. } | ModifiersChanged(_) => {
                    self.context().wants_keyboard_input()
                }

                MouseWheel { .. } | MouseInput { .. } => self.context().wants_pointer_input(),

                CursorMoved { .. } => self.context().is_using_pointer(),

                Touch { .. } => self.context().is_using_pointer(),

                _ => false,
            },

            _ => false,
        }
    }

    /// Updates the internal time for egui used for animations. `elapsed_seconds` should be the seconds since some point in time (for example application start).
    pub fn update_time(&mut self, elapsed_seconds: f64) {
        self.raw_input.time = Some(elapsed_seconds);
    }

    /// Starts a new frame by providing a new `Ui` instance to write into.
    pub fn begin_frame(&mut self) {
        self.context.begin_frame(self.raw_input.take());
    }

    /// Ends the frame. Returns what has happened as `Output` and gives you the draw instructions
    /// as `PaintJobs`. If the optional `window` is set, it will set the cursor key based on
    /// egui's instructions.
    pub fn end_frame(&mut self, window: Option<&winit::window::Window>) -> egui::FullOutput {
        // otherwise the below line gets flagged by clippy if both clipboard and webbrowser features are disabled
        #[allow(clippy::let_and_return)]
        let output = self.context.end_frame();

        if let Some(window) = window {
            if let Some(cursor_icon) = egui_to_winit_cursor_icon(output.platform_output.cursor_icon)
            {
                window.set_cursor_visible(true);
                // if the pointer is located inside the window, set cursor icon
                if self.pointer_pos.is_some() {
                    window.set_cursor_icon(cursor_icon);
                }
            } else {
                window.set_cursor_visible(false);
            }
        }

        #[cfg(feature = "clipboard")]
        handle_clipboard(&output.platform_output, self.clipboard.as_mut());

        #[cfg(feature = "webbrowser")]
        handle_links(&output.platform_output);

        output
    }

    /// Returns the internal egui context.
    pub fn context(&self) -> Context {
        self.context.clone()
    }

    /// Returns a mutable reference to the raw input that will be passed to egui
    /// the next time [`Self::begin_frame`] is called
    pub fn raw_input_mut(&mut self) -> &mut egui::RawInput {
        &mut self.raw_input
    }
}

/// Translates winit to egui keycodes.
#[inline]
fn winit_to_egui_physical_key_code(key: PhysicalKey) -> Option<egui::Key> {
    match key {
        PhysicalKey::Code(physical_code) => {
            match physical_code {
                KeyCode::Backslash => Some(Key::Backslash),
                KeyCode::Comma => Some(Key::Comma),
                KeyCode::Minus => Some(Key::Minus),
                KeyCode::Period => Some(Key::Period),
                KeyCode::Semicolon => Some(Key::Semicolon),
                KeyCode::AltLeft => Some(Key::ArrowLeft),
                KeyCode::AltRight => Some(Key::ArrowRight),
                KeyCode::Backspace => Some(Key::Backspace),
                KeyCode::Enter => Some(Key::Enter),
                KeyCode::Space => Some(Key::Space),
                KeyCode::Tab => Some(Key::Tab),
                KeyCode::Delete => Some(Key::Delete),
                KeyCode::End => Some(Key::End),
                KeyCode::Home => Some(Key::Home),
                KeyCode::Insert => Some(Key::Insert),
                KeyCode::PageDown => Some(Key::PageDown),
                KeyCode::PageUp => Some(Key::PageUp),
                KeyCode::ArrowDown => Some(Key::ArrowDown),
                KeyCode::ArrowLeft => Some(Key::ArrowLeft),
                KeyCode::ArrowRight => Some(Key::ArrowRight),
                KeyCode::ArrowUp => Some(Key::ArrowUp),
                KeyCode::Numpad0 => Some(Key::Num0),
                KeyCode::Numpad1 => Some(Key::Num1),
                KeyCode::Numpad2 => Some(Key::Num2),
                KeyCode::Numpad3 => Some(Key::Num3),
                KeyCode::Numpad4 => Some(Key::Num4),
                KeyCode::Numpad5 => Some(Key::Num5),
                KeyCode::Numpad6 => Some(Key::Num6),
                KeyCode::Numpad7 => Some(Key::Num7),
                KeyCode::Numpad8 => Some(Key::Num8),
                KeyCode::Numpad9 => Some(Key::Num9),
                KeyCode::Escape => Some(Key::Escape),
                KeyCode::Copy => Some(Key::Copy),
                KeyCode::Cut => Some(Key::Cut),
                KeyCode::Paste => Some(Key::Paste),
                KeyCode::F1 => Some(Key::F1),
                KeyCode::F2 => Some(Key::F2),
                KeyCode::F3 => Some(Key::F3),
                KeyCode::F4 => Some(Key::F4),
                KeyCode::F5 => Some(Key::F5),
                KeyCode::F6 => Some(Key::F6),
                KeyCode::F7 => Some(Key::F7),
                KeyCode::F8 => Some(Key::F8),
                KeyCode::F9 => Some(Key::F9),
                KeyCode::F10 => Some(Key::F10),
                KeyCode::F11 => Some(Key::F11),
                KeyCode::F12 => Some(Key::F12),
                KeyCode::F13 => Some(Key::F13),
                KeyCode::F14 => Some(Key::F14),
                KeyCode::F15 => Some(Key::F15),
                KeyCode::F16 => Some(Key::F16),
                KeyCode::F17 => Some(Key::F17),
                KeyCode::F18 => Some(Key::F18),
                KeyCode::F19 => Some(Key::F19),
                KeyCode::F20 => Some(Key::F20),
                _ => None,
            }
        }
        PhysicalKey::Unidentified(c) => None,
    }
}
/// Translates winit to egui keycodes.
#[inline]
fn winit_to_egui_key_code(key: winit::keyboard::Key) -> Option<egui::Key> {
    match key {
        winit::keyboard::Key::Named(name) => match name {
            NamedKey::Enter => Some(Key::Enter),
            NamedKey::Tab => Some(Key::Tab),
            NamedKey::Space => Some(Key::Space),
            NamedKey::ArrowDown => Some(Key::ArrowDown),
            NamedKey::ArrowLeft => Some(Key::ArrowLeft),
            NamedKey::ArrowRight => Some(Key::ArrowRight),
            NamedKey::ArrowUp => Some(Key::ArrowUp),
            NamedKey::End => Some(Key::End),
            NamedKey::Home => Some(Key::Home),
            NamedKey::PageDown => Some(Key::PageDown),
            NamedKey::PageUp => Some(Key::PageUp),
            NamedKey::Backspace => Some(Key::Backspace),
            NamedKey::Copy => Some(Key::Copy),
            NamedKey::Cut => Some(Key::Cut),
            NamedKey::Delete => Some(Key::Delete),
            NamedKey::Insert => Some(Key::Insert),
            NamedKey::Paste => Some(Key::Paste),
            NamedKey::Escape => Some(Key::Escape),
            NamedKey::Execute => Some(Key::Enter),
            NamedKey::F1 => Some(Key::F1),
            NamedKey::F2 => Some(Key::F2),
            NamedKey::F3 => Some(Key::F3),
            NamedKey::F4 => Some(Key::F4),
            NamedKey::F5 => Some(Key::F5),
            NamedKey::F6 => Some(Key::F6),
            NamedKey::F7 => Some(Key::F7),
            NamedKey::F8 => Some(Key::F8),
            NamedKey::F9 => Some(Key::F9),
            NamedKey::F10 => Some(Key::F10),
            NamedKey::F11 => Some(Key::F11),
            NamedKey::F12 => Some(Key::F12),
            NamedKey::F13 => Some(Key::F13),
            NamedKey::F14 => Some(Key::F14),
            NamedKey::F15 => Some(Key::F15),
            NamedKey::F16 => Some(Key::F16),
            NamedKey::F17 => Some(Key::F17),
            NamedKey::F18 => Some(Key::F18),
            NamedKey::F19 => Some(Key::F19),
            NamedKey::F20 => Some(Key::F20),
            _ => None
        }
        winit::keyboard::Key::Character(c) => Key::from_name(c.as_str()),
        winit::keyboard::Key::Unidentified(k) => None,
        winit::keyboard::Key::Dead(None) => None,
        winit::keyboard::Key::Dead(Some(c)) => Key::from_name(&String::from(c)),
    }
}

/// Translates winit to egui modifier keys.
#[inline]
fn winit_to_egui_modifiers(modifiers: ModifiersState) -> egui::Modifiers {
    egui::Modifiers {
        alt: modifiers.alt_key(),
        ctrl: modifiers.control_key(),
        shift: modifiers.shift_key(),
        #[cfg(target_os = "macos")]
        mac_cmd: modifiers.logo(),
        #[cfg(target_os = "macos")]
        command: modifiers.logo(),
        #[cfg(not(target_os = "macos"))]
        mac_cmd: false,
        #[cfg(not(target_os = "macos"))]
        command: modifiers.control_key(),
    }
}

#[inline]
fn egui_to_winit_cursor_icon(icon: egui::CursorIcon) -> Option<winit::window::CursorIcon> {
    use egui::CursorIcon::*;

    match icon {
        Default => Some(CursorIcon::Default),
        ContextMenu => Some(CursorIcon::ContextMenu),
        Help => Some(CursorIcon::Help),
        PointingHand => Some(CursorIcon::Pointer),
        Progress => Some(CursorIcon::Progress),
        Wait => Some(CursorIcon::Wait),
        Cell => Some(CursorIcon::Cell),
        Crosshair => Some(CursorIcon::Crosshair),
        Text => Some(CursorIcon::Text),
        VerticalText => Some(CursorIcon::VerticalText),
        Alias => Some(CursorIcon::Alias),
        Copy => Some(CursorIcon::Copy),
        Move => Some(CursorIcon::Move),
        NoDrop => Some(CursorIcon::NoDrop),
        NotAllowed => Some(CursorIcon::NotAllowed),
        Grab => Some(CursorIcon::Grab),
        Grabbing => Some(CursorIcon::Grabbing),
        AllScroll => Some(CursorIcon::AllScroll),
        ResizeHorizontal => Some(CursorIcon::EwResize),
        ResizeNeSw => Some(CursorIcon::NeswResize),
        ResizeNwSe => Some(CursorIcon::NwseResize),
        ResizeVertical => Some(CursorIcon::NsResize),
        ResizeEast => Some(CursorIcon::EResize),
        ResizeSouthEast => Some(CursorIcon::SeResize),
        ResizeSouth => Some(CursorIcon::SResize),
        ResizeSouthWest => Some(CursorIcon::SwResize),
        ResizeWest => Some(CursorIcon::WResize),
        ResizeNorthWest => Some(CursorIcon::NwResize),
        ResizeNorth => Some(CursorIcon::NResize),
        ResizeNorthEast => Some(CursorIcon::NeResize),
        ResizeColumn => Some(CursorIcon::ColResize),
        ResizeRow => Some(CursorIcon::RowResize),
        ZoomIn => Some(CursorIcon::ZoomIn),
        ZoomOut => Some(CursorIcon::ZoomOut),
        None => Option::None,
    }
}

/// We only want printable characters and ignore all special keys.
#[inline]
fn is_printable(chr: char) -> bool {
    let is_in_private_use_area = ('\u{e000}'..='\u{f8ff}').contains(&chr)
        || ('\u{f0000}'..='\u{ffffd}').contains(&chr)
        || ('\u{100000}'..='\u{10fffd}').contains(&chr);

    !is_in_private_use_area && !chr.is_ascii_control()
}
