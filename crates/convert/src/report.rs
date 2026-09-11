use crate::plan::PathTaken;

#[derive(Clone, Debug)]
pub struct Report {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub width_in: Option<u32>,
    pub height_in: Option<u32>,
    pub width_out: Option<u32>,
    pub height_out: Option<u32>,
    pub duration_in: Option<f64>,
    pub duration_out: Option<f64>,
    pub path: PathTaken,
    pub notes: String,
}

impl Report {
    pub fn saved_pct(&self) -> f32 {
        if self.bytes_in == 0 {
            return 0.0;
        }
        let delta = self.bytes_in as f64 - self.bytes_out as f64;
        (delta / self.bytes_in as f64 * 100.0) as f32
    }

    pub fn grew(&self) -> bool {
        self.bytes_out > self.bytes_in
    }
}
