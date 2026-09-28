use crate::{PathTaken, Probe};

#[derive(Clone, Debug)]
pub struct Report {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub width_out: Option<u32>,
    pub height_out: Option<u32>,
    pub duration_in: Option<f64>,
    pub duration_out: Option<f64>,
    pub path: PathTaken,
    pub notes: String,
}

impl Report {
    pub(crate) fn new(probe: &Probe, path: PathTaken, bytes_out: u64, notes: String) -> Self {
        Self {
            bytes_in: probe.bytes,
            bytes_out,
            width_out: probe.width,
            height_out: probe.height,
            duration_in: probe.duration,
            duration_out: probe.duration,
            path,
            notes,
        }
    }

    pub fn grew(&self) -> bool {
        self.bytes_out > self.bytes_in
    }
}
