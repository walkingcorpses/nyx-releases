// assoc.rs - makes Nyx the handler for .torrent files and magnet: links without an installer,
// by writing per-user registry keys under HKCU\Software\Classes. Nothing needs admin rights.

use std::io;
use std::path::Path;

use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS};
use winreg::RegKey;

/// Production location of per-user file associations.
const CLASSES: &str = "Software\\Classes";
/// ProgID used for .torrent files.
const PROG_ID: &str = "Nyx.torrent";

/// Registers Nyx for .torrent and magnet: under the real `HKCU\Software\Classes`.
pub fn register(exe: &Path) -> io::Result<()> {
    register_under(CLASSES, exe)
}

/// Removes what `register` wrote.
pub fn unregister() -> io::Result<()> {
    unregister_under(CLASSES)
}

/// Writes the keys under `root` (a path below HKCU). Production passes `Software\Classes`;
/// tests pass a throwaway key so the real associations are never touched.
pub fn register_under(root: &str, exe: &Path) -> io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let classes = hkcu.create_subkey(root)?.0;
    let cmd = format!("\"{}\" \"%1\"", exe.display());

    // .torrent -> Nyx.torrent -> open command
    classes.create_subkey(".torrent")?.0.set_value("", &PROG_ID)?;
    let prog = classes.create_subkey(PROG_ID)?.0;
    prog.set_value("", &"Torrent file")?;
    prog.create_subkey("shell\\open\\command")?.0.set_value("", &cmd)?;

    // magnet: URL protocol -> open command
    let magnet = classes.create_subkey("magnet")?.0;
    magnet.set_value("", &"URL:Magnet Protocol")?;
    magnet.set_value("URL Protocol", &"")?;
    magnet.create_subkey("shell\\open\\command")?.0.set_value("", &cmd)?;
    Ok(())
}

/// Deletes the keys `register_under` created below `root`. Missing keys are fine.
pub fn unregister_under(root: &str) -> io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(classes) = hkcu.open_subkey_with_flags(root, KEY_ALL_ACCESS) else { return Ok(()) };
    for key in [".torrent", PROG_ID, "magnet"] {
        match classes.delete_subkey_all(key) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assoc_register_writes_expected_keys() {
        let root = format!("Software\\NyxTest\\{}\\Classes", std::process::id());
        let exe = Path::new("D:\\apps\\nyx.exe");
        register_under(&root, exe).unwrap();

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let classes = hkcu.open_subkey(&root).unwrap();
        let ext: String = classes.open_subkey(".torrent").unwrap().get_value("").unwrap();
        assert_eq!(ext, PROG_ID);
        let cmd: String = classes.open_subkey("Nyx.torrent\\shell\\open\\command").unwrap().get_value("").unwrap();
        assert_eq!(cmd, "\"D:\\apps\\nyx.exe\" \"%1\"");
        let mcmd: String = classes.open_subkey("magnet\\shell\\open\\command").unwrap().get_value("").unwrap();
        assert_eq!(mcmd, cmd);
        let proto: String = classes.open_subkey("magnet").unwrap().get_value("URL Protocol").unwrap();
        assert_eq!(proto, "");

        unregister_under(&root).unwrap();
        assert!(classes.open_subkey(".torrent").is_err());
        let _ = hkcu.delete_subkey_all(format!("Software\\NyxTest\\{}", std::process::id()));
    }
}
