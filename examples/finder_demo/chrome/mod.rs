pub(crate) mod connect;
pub(crate) mod criteria_bar;
pub(crate) mod path_bar;
pub(crate) mod scope_bar;
pub(crate) mod sftp;
pub(crate) mod sheet;
pub(crate) mod sidebar;
pub(crate) mod status_bar;
pub(crate) mod surface;
pub(crate) mod toolbar;

fn strs(list: &[String]) -> Vec<&str> {
    list.iter().map(String::as_str).collect()
}
