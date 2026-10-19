use std::path::PathBuf;

use finder_fs::viewer;
use serde_json::json;

use crate::args::Args;
use crate::out::{self, Out};
use crate::{OK, Stop};

const APP_ID: &str = "dev.cafekery.founder";
const APP: &str = "/Applications/Found'er.app";

pub fn viewer(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let action = args.shift().unwrap_or_default();
    let id = args.text("id").unwrap_or_else(|| APP_ID.into());
    let app = args.path("app").unwrap_or_else(|| PathBuf::from(APP));
    args.done("viewer")?;
    let failed = |error: std::io::Error| Stop::Refused(error.to_string());
    match action.as_str() {
        "status" => {}
        "on" => viewer::on(&id, &app).map_err(failed)?,
        "off" => viewer::off(&id).map_err(failed)?,
        _ => return Err(Stop::Usage("viewer takes status, on or off".into())),
    }
    let status = viewer::status(&id).map_err(failed)?;
    let shown = status.viewer.clone().unwrap_or_else(|| "Finder".into());
    if out.json {
        out.emit(json!({
            "event": "viewer",
            "viewer": status.viewer,
            "guard": status.guard.as_ref().map(|guard| json!({
                "bundle": out::path(&guard.bundle),
                "previous": guard.previous,
                "loaded": guard.is_loaded,
            })),
        }));
    } else {
        out.say(&format!("file viewer: {shown}"));
        if let Some(guard) = &status.guard {
            out.say(&format!(
                "guard: watches {}, restores {}",
                guard.bundle.display(),
                guard.previous.as_deref().unwrap_or("Finder")
            ));
        }
    }
    Ok(OK)
}
