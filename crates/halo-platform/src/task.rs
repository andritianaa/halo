//! Elevated start at sign-in through the Task Scheduler.
//!
//! Windows skips programs that require elevation in the `Run` registry key, so an
//! always-admin app has to start from a logon task with "highest privileges". The
//! task is defined in XML to lift the scheduler's defaults that would otherwise stop
//! Halo (no start on battery, killed after 72 h).

use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn schtasks(args: &[&str]) -> bool {
    Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Task definition: at this user's sign-in, highest privileges, no time limit.
pub fn logon_task_xml(user: &str, exe: &str, args: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Halo — start at sign-in</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command><Arguments>{args}</Arguments></Exec></Actions>
</Task>
"#,
        user = xml_escape(user),
        exe = xml_escape(exe),
        args = xml_escape(args),
    )
}

/// `DOMAIN\user` of the signed-in user.
fn current_user() -> Option<String> {
    let user = std::env::var("USERNAME").ok()?;
    Some(match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
        _ => user,
    })
}

/// Create or replace the logon task. Needs an elevated process.
pub fn create_logon_task(name: &str, exe: &std::path::Path, args: &str) -> bool {
    let Some(user) = current_user() else { return false };
    let xml = logon_task_xml(&user, &exe.display().to_string(), args);
    // schtasks reads the XML file as UTF-16 with a BOM.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(|u| u.to_le_bytes()));
    let path = std::env::temp_dir().join(format!("{name}-task.xml"));
    if std::fs::write(&path, bytes).is_err() {
        return false;
    }
    let ok = schtasks(&["/Create", "/TN", name, "/XML", &path.display().to_string(), "/F"]);
    let _ = std::fs::remove_file(&path);
    ok
}

pub fn task_exists(name: &str) -> bool {
    schtasks(&["/Query", "/TN", name])
}

pub fn delete_task(name: &str) -> bool {
    !task_exists(name) || schtasks(&["/Delete", "/TN", name, "/F"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_has_no_time_limit_runs_on_battery_and_escapes() {
        let x = logon_task_xml(r"PC\Ana", r"C:\Program Files\Halo\halo.exe", "--minimized");
        assert!(x.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(x.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(x.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(x.contains(r"<UserId>PC\Ana</UserId>"));
        assert!(logon_task_xml("a&b", "x", "y").contains("a&amp;b"));
    }
}
