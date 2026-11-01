//! Audio drivers and devices (thin layer over cpal), shared by the Settings window and the app's start-up code.
use cpal::traits::{DeviceTrait, HostTrait};

/// Names of the drivers ("hosts") available on this system, e.g. WASAPI, ASIO, ALSA, JACK, CoreAudio.
pub fn host_names() -> Vec<String> {
    cpal::available_hosts().into_iter().map(|h| h.name().to_string()).collect()
}

pub fn default_host_name() -> String {
    cpal::default_host().id().name().to_string()
}

/// The driver called `name`; the system default when `name` is `None` or not available any more.
pub fn open_host(name: Option<&str>) -> cpal::Host {
    if let Some(n) = name {
        if let Some(id) = cpal::available_hosts().into_iter().find(|h| h.name() == n) {
            if let Ok(h) = cpal::host_from_id(id) {
                return h;
            }
        }
        eprintln!("Audio driver \"{n}\" is not available, using the default");
    }
    cpal::default_host()
}

pub fn output_names(host: &cpal::Host) -> Vec<String> {
    host.output_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
}

pub fn input_names(host: &cpal::Host) -> Vec<String> {
    host.input_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
}

pub fn default_output_name(host: &cpal::Host) -> Option<String> {
    host.default_output_device().and_then(|d| d.name().ok())
}

pub fn default_input_name(host: &cpal::Host) -> Option<String> {
    host.default_input_device().and_then(|d| d.name().ok())
}

/// The output device called `name`, or the driver's default one.
pub fn output_device(host: &cpal::Host, name: Option<&str>) -> Option<cpal::Device> {
    if let Some(n) = name {
        if let Some(d) = host.output_devices().ok().and_then(|mut it| it.find(|d| d.name().map_or(false, |x| x == n))) {
            return Some(d);
        }
        eprintln!("Output device \"{n}\" not found, using the default");
    }
    host.default_output_device()
}

/// The input device called `name`, or the driver's default one.
pub fn input_device(host: &cpal::Host, name: Option<&str>) -> Option<cpal::Device> {
    if let Some(n) = name {
        if let Some(d) = host.input_devices().ok().and_then(|mut it| it.find(|d| d.name().map_or(false, |x| x == n))) {
            return Some(d);
        }
        eprintln!("Input device \"{n}\" not found, using the default");
    }
    host.default_input_device()
}
