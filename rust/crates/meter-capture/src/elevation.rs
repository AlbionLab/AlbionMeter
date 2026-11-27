//! `WindowsElevation`: the raw socket capture needs an administrator token.

/// The command line `RestartElevated` passes on: plain arguments as they are, the rest quoted with inner quotes
/// escaped the way .NET's command line parser reads them back.
pub fn command_line<S: AsRef<str>>(args: &[S]) -> String {
    args.iter()
        .map(|argument| {
            let argument = argument.as_ref();
            if !argument.is_empty()
                && !argument
                    .chars()
                    .any(|character| character.is_whitespace() || character == '"')
            {
                argument.to_owned()
            } else {
                format!(
                    "\"{}\"",
                    argument.replace("\\\"", "\\\\\"").replace('"', "\\\"")
                )
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(windows)]
pub use self::windows::{is_administrator, restart_elevated};

#[cfg(windows)]
mod windows {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{
        CheckTokenMembership, CreateWellKnownSid, PSID, SECURITY_MAX_SID_SIZE,
        WinBuiltinAdministratorsSid,
    };
    use windows::Win32::UI::Shell::{SEE_MASK_DEFAULT, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{BOOL, HSTRING, PCWSTR, w};

    /// `WindowsPrincipal.IsInRole(Administrator)`: an elevated token in the Administrators group.
    pub fn is_administrator() -> bool {
        let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
        let mut size = sid.len() as u32;
        let sid = PSID(sid.as_mut_ptr().cast());
        let mut member = BOOL(0);
        // SAFETY: `sid` points at a buffer of `size` bytes that outlives both calls.
        unsafe {
            CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(sid), &mut size).is_ok()
                && CheckTokenMembership(Some(HANDLE::default()), sid, &mut member).is_ok()
                && member.as_bool()
        }
    }

    /// Starts this executable again through the UAC prompt; `false` when it could not (the player said no).
    pub fn restart_elevated<S: AsRef<str>>(args: &[S]) -> bool {
        let Ok(program) = std::env::current_exe() else {
            return false;
        };
        let file = HSTRING::from(program.as_os_str());
        let parameters = HSTRING::from(super::command_line(args));
        let directory = program
            .parent()
            .map(|parent| HSTRING::from(parent.as_os_str()))
            .unwrap_or_default();
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_DEFAULT,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(parameters.as_ptr()),
            lpDirectory: PCWSTR(directory.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        // SAFETY: every string in `info` lives until the call returns.
        unsafe { ShellExecuteExW(&mut info).is_ok() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_like_restart_elevated() {
        assert_eq!(command_line(&["--port", "5000"]), "--port 5000");
        assert_eq!(command_line(&[""]), "\"\"");
        assert_eq!(
            command_line(&[r"C:\Program Files\Albion"]),
            r#""C:\Program Files\Albion""#
        );
        assert_eq!(command_line(&[r#"say "hi""#]), r#""say \"hi\"""#);
        assert_eq!(command_line(&[r#"a\"b"#]), r#""a\\\"b""#);
        assert_eq!(command_line::<&str>(&[]), "");
    }
}
