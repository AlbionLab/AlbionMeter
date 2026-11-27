//! `--screen <name> --screenshot <png>`: renders one fixture screen with the software renderer into a PNG of the exact
//! window size, without a window or an event loop (for rust/scripts/ui-parity.mjs).

use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};

use crate::controller::Controller;
use crate::fixture;
use crate::i18n::Language;
use crate::{AppWindow, MeterState, Shell, Theme};

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> std::time::Duration {
        self.start.elapsed()
    }
}

pub fn run(fixture_path: &Path, name: &str, output: &Path) -> Result<(), String> {
    let screen = fixture::screen(fixture_path, name)?;
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless { window: window.clone(), start: Instant::now() })).map_err(|error| error.to_string())?;
    let app = AppWindow::new().map_err(|error| error.to_string())?;
    let (width, height) = (screen.width as u32, screen.height as u32);
    window.set_size(slint::PhysicalSize::new(width, height));
    let (sans, mono) = crate::desktop::fonts();
    app.global::<Theme>().set_sans(sans.into());
    app.global::<Theme>().set_mono(mono.into());
    let (commands, _received) = std::sync::mpsc::channel();
    let language = if screen.ui.language == "pt" { Language::Pt } else { Language::En };
    let controller = Controller::new(&app, commands, language);
    controller.borrow_mut().persist = false;
    for event in screen.events {
        controller.borrow_mut().handle(event);
    }
    app.global::<Shell>().set_view(screen.ui.view.as_str().into());
    app.global::<MeterState>().invoke_metric_changed(screen.ui.metric.as_str().into());
    app.global::<MeterState>().set_confirm(screen.ui.confirm.as_str().into());
    app.show().map_err(|error| error.to_string())?;
    let mut buffer = vec![PremultipliedRgbaColor::default(); (width * height) as usize];
    // Bindings that depend on the first layout (the window width) settle over a few frames.
    for _ in 0..4 {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut buffer, width as usize);
        });
    }
    let rgba: Vec<u8> = buffer.iter().flat_map(|pixel| [pixel.red, pixel.green, pixel.blue, 255]).collect();
    if let Some(parent) = output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(output, crate::screenshots::png(width, height, &rgba)).map_err(|error| format!("{}: {error}", output.display()))?;
    drop(controller);
    Ok(())
}
