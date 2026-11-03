//! macOS settings through the `defaults` command. Arguments are fixed and passed without a
//! shell. The parsers and argument builders are pure so they are tested on every OS.

use super::journal::Val;

/// Formats a float the same way for what we read and what we write ("0.001", "0", "2.5").
pub fn float_str(s: &str) -> Option<String> {
    let f: f64 = s.trim().parse().ok()?;
    if !f.is_finite() {
        return None;
    }
    Some(format!("{f}"))
}

/// True when `defaults read` failed only because the key (or domain) is not set.
pub fn is_missing(stderr: &str) -> bool {
    stderr.contains("does not exist")
}

/// Turns `defaults read` output into a value. `ty` is bool, int, float or string.
pub fn parse_defaults_read(out: &str, ty: &str) -> Val {
    let t = out.trim();
    match ty {
        "bool" | "int" => {
            let lower = t.to_ascii_lowercase();
            if lower == "true" || lower == "yes" {
                return Val::Dword(1);
            }
            if lower == "false" || lower == "no" {
                return Val::Dword(0);
            }
            match t.parse::<i64>() {
                Ok(n) if (0..=i64::from(u32::MAX)).contains(&n) => Val::Dword(n as u32),
                _ => Val::Other,
            }
        }
        "float" => match float_str(t) {
            Some(s) => Val::Str(s),
            None => Val::Other,
        },
        "string" => {
            if t.starts_with('(') || t.starts_with('{') {
                return Val::Other;
            }
            Val::Str(t.trim_matches('"').to_string())
        }
        _ => Val::Other,
    }
}

fn safe_name(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && !s.contains('\0')
}

/// The arguments for `defaults` that write `v`, or delete the key for `Absent`.
pub fn write_args(domain: &str, key: &str, ty: &str, v: &Val) -> Result<Vec<String>, String> {
    if !safe_name(domain) || !safe_name(key) {
        return Err("Not a valid setting name.".into());
    }
    let s = |x: &str| x.to_string();
    if *v == Val::Absent {
        return Ok(vec![s("delete"), s(domain), s(key)]);
    }
    let value = match (ty, v) {
        ("bool", Val::Dword(n)) => ("-bool", if *n != 0 { "true".to_string() } else { "false".to_string() }),
        ("int", Val::Dword(n)) => ("-int", n.to_string()),
        ("float", Val::Str(x)) => ("-float", float_str(x).ok_or("Not a number.")?),
        ("string", Val::Str(x)) => ("-string", x.clone()),
        _ => return Err("Cannot set this setting to that value.".into()),
    };
    Ok(vec![s("write"), s(domain), s(key), s(value.0), value.1])
}

#[cfg(target_os = "macos")]
fn run(args: &[String]) -> Result<(String, String, bool), String> {
    let o = std::process::Command::new("/usr/bin/defaults")
        .args(args)
        .output()
        .map_err(|e| format!("Could not run defaults: {e}"))?;
    Ok((
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
        o.status.success(),
    ))
}

#[cfg(target_os = "macos")]
pub fn read(t: &super::journal::Target) -> Result<Val, String> {
    let super::journal::Target::Defaults { domain, key, ty } = t else {
        return Err("Not a macOS setting.".into());
    };
    if !safe_name(domain) || !safe_name(key) {
        return Err("Not a valid setting name.".into());
    }
    let (out, err, ok) = run(&["read".into(), domain.clone(), key.clone()])?;
    if ok {
        Ok(parse_defaults_read(&out, ty))
    } else if is_missing(&err) {
        Ok(Val::Absent)
    } else {
        Err(err.trim().to_string())
    }
}

#[cfg(target_os = "macos")]
pub fn write(t: &super::journal::Target, v: &Val) -> Result<(), String> {
    let super::journal::Target::Defaults { domain, key, ty } = t else {
        return Err("Not a macOS setting.".into());
    };
    let args = write_args(domain, key, ty, v)?;
    let (_, err, ok) = run(&args)?;
    if ok || (*v == Val::Absent && is_missing(&err)) {
        Ok(())
    } else {
        Err(format!("macOS refused the change: {}", err.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_parsing() {
        assert_eq!(parse_defaults_read("1\n", "bool"), Val::Dword(1));
        assert_eq!(parse_defaults_read("0\n", "bool"), Val::Dword(0));
        assert_eq!(parse_defaults_read("true", "bool"), Val::Dword(1));
        assert_eq!(parse_defaults_read("NO", "bool"), Val::Dword(0));
        assert_eq!(parse_defaults_read("15\n", "int"), Val::Dword(15));
        assert_eq!(parse_defaults_read("-1", "int"), Val::Other);
        assert_eq!(parse_defaults_read("0.001\n", "float"), Val::Str("0.001".into()));
        assert_eq!(parse_defaults_read("0", "float"), Val::Str("0".into()));
        assert_eq!(parse_defaults_read("scale\n", "string"), Val::Str("scale".into()));
        assert_eq!(parse_defaults_read("\"Assistant Enabled\"", "string"), Val::Str("Assistant Enabled".into()));
        assert_eq!(parse_defaults_read("(\n a\n)", "string"), Val::Other);
        assert_eq!(parse_defaults_read("x", "int"), Val::Other);
    }

    #[test]
    fn missing_key_detection() {
        assert!(is_missing("\nThe domain/default pair of (com.apple.dock, foo) does not exist\n"));
        assert!(!is_missing("permission denied"));
    }

    #[test]
    fn write_argument_building() {
        let w = |ty: &str, v: Val| write_args("com.apple.dock", "k", ty, &v);
        assert_eq!(w("bool", Val::Dword(1)).unwrap(), ["write", "com.apple.dock", "k", "-bool", "true"]);
        assert_eq!(w("bool", Val::Dword(0)).unwrap()[4], "false");
        assert_eq!(w("int", Val::Dword(2)).unwrap()[3..], ["-int", "2"]);
        assert_eq!(w("float", Val::Str("0.30".into())).unwrap()[3..], ["-float", "0.3"]);
        assert_eq!(w("string", Val::Str("scale".into())).unwrap()[3..], ["-string", "scale"]);
        assert_eq!(w("bool", Val::Absent).unwrap(), ["delete", "com.apple.dock", "k"]);
        assert!(w("bool", Val::Str("x".into())).is_err());
        assert!(w("float", Val::Str("abc".into())).is_err());
        assert!(write_args("-g", "k", "bool", &Val::Dword(1)).is_err());
        assert!(write_args("d", "", "bool", &Val::Dword(1)).is_err());
    }
}
