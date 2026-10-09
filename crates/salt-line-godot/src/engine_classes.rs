//! The engine classes Salt Line needs, checked when the library loads.
//!
//! Android ships on a Godot engine built without the classes this game never
//! uses (`tools/godot_build_profile.json`, `docs/godot-templates.md`), and
//! gdext resolves engine methods lazily (`lazy-function-tables`), so a class
//! missing from that build would otherwise surface as a panic deep in some
//! code path. [`check`] looks every class here up at startup instead and
//! fails at once, naming what is missing.
//!
//! The list is the one place to record a new engine dependency. A test reads
//! every `godot::classes::` path in this crate's source and fails if a class
//! it names is not here, so adding one shows up in review next to the change
//! that needs it (and is a reminder to check the build profile keeps it).

use godot::classes::ClassDb;
use godot::prelude::*;

/// Godot class names (as `ClassDB` knows them, so `OS`, not `Os`).
pub const REQUIRED: &[&str] = &[
    // Imported from `godot::classes` by this crate (the test checks these).
    "AudioStream",
    "AudioStreamPlayer",
    "CanvasItem",
    "ClassDB",
    "DirAccess",
    "DisplayServer",
    "Engine",
    "FileAccess",
    "Image",
    "ImageTexture",
    "Input",
    "InputEvent",
    "InputEventJoypadButton",
    "InputEventJoypadMotion",
    "InputEventKey",
    "InputEventMouseMotion",
    "InputEventScreenDrag",
    "InputEventScreenTouch",
    "Node2D",
    "OS",
    "ProjectSettings",
    "RenderingServer",
    "ResourceLoader",
    "Texture2D",
    "Time",
    "Window",
    // Reached without being named: what the imported sprites and sounds load
    // as, the scene tree the node lives in, and the scripts in godot/ and the
    // XDG Portals addon.
    "AudioStreamWAV",
    "CompressedTexture2D",
    "SceneTree",
    "Viewport",
    "GDScript",
];

/// The [`REQUIRED`] classes this engine does not have.
pub fn missing() -> Vec<&'static str> {
    let db = ClassDb::singleton();
    REQUIRED
        .iter()
        .copied()
        .filter(|name| !db.class_exists(&StringName::from(*name)))
        .collect()
}

/// Panics, naming them, if the engine lacks any [`REQUIRED`] class.
pub fn check() {
    let missing = missing();
    if !missing.is_empty() {
        let msg = format!(
            "this Godot engine build lacks classes Salt Line uses: {}. If it is the \
             stripped engine, re-enable them in tools/godot_build_profile.json \
             (docs/godot-templates.md, \"Adding something back\").",
            missing.join(", ")
        );
        godot_error!("{msg}");
        panic!("{msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::REQUIRED;
    use std::collections::BTreeSet;
    use std::path::Path;

    /// gdext's Rust names that differ from Godot's.
    fn godot_name(rust: &str) -> String {
        match rust {
            "Os" => "OS".into(),
            "ClassDb" => "ClassDB".into(),
            other => other.into(),
        }
    }

    fn snake_to_camel(s: &str) -> String {
        s.split('_')
            .map(|part| {
                let mut c = part.chars();
                c.next()
                    .map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
            })
            .collect()
    }

    /// The class a `godot::classes::` path item refers to: `Image`,
    /// `image::Format` (an enum in Image's module), `INode2D` (Node2D's
    /// virtual-method trait), `notify::CanvasItemNotification`.
    fn class_of(item: &str) -> Option<String> {
        let item = item.trim();
        let parts: Vec<&str> = item.split("::").map(str::trim).collect();
        let name = match parts.as_slice() {
            ["notify", n, ..] => n.strip_suffix("Notification")?.to_string(),
            [module, ..] if module.chars().next()?.is_lowercase() => snake_to_camel(module),
            [n, ..] => {
                let n = n.trim_end_matches(';');
                match n.strip_prefix('I') {
                    Some(rest) if rest.chars().next().is_some_and(char::is_uppercase) => {
                        rest.to_string()
                    }
                    _ => n.to_string(),
                }
            }
            [] => return None,
        };
        let name: String = name.chars().take_while(|c| c.is_alphanumeric()).collect();
        (!name.is_empty()).then(|| godot_name(&name))
    }

    /// Every class named through `godot::classes::` in this crate's source.
    fn imported_classes() -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let mut rest = text.as_str();
                while let Some(at) = rest.find("godot::classes::") {
                    rest = &rest[at + "godot::classes::".len()..];
                    if let Some(group) = rest.strip_prefix('{') {
                        let end = group.find('}').unwrap();
                        for item in group[..end].split(',') {
                            out.extend(class_of(item));
                        }
                    } else {
                        let end = rest
                            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
                            .unwrap_or(rest.len());
                        out.extend(class_of(&rest[..end]));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn every_imported_class_is_listed() {
        let imported = imported_classes();
        assert!(
            imported.contains("Node2D"),
            "the scan found nothing: {imported:?}"
        );
        let missing: Vec<_> = imported
            .iter()
            .filter(|c| !REQUIRED.contains(&c.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "engine classes used but not in engine_classes::REQUIRED: {missing:?} \
             (add them, and check tools/godot_build_profile.json keeps them)"
        );
    }

    #[test]
    fn class_paths_map_to_godot_names() {
        assert_eq!(class_of("image::Format::RGBA8").as_deref(), Some("Image"));
        assert_eq!(
            class_of("window::ContentScaleAspect").as_deref(),
            Some("Window")
        );
        assert_eq!(
            class_of("file_access::ModeFlags").as_deref(),
            Some("FileAccess")
        );
        assert_eq!(
            class_of("notify::CanvasItemNotification").as_deref(),
            Some("CanvasItem")
        );
        assert_eq!(class_of(" INode2D").as_deref(), Some("Node2D"));
        assert_eq!(class_of("Os").as_deref(), Some("OS"));
        assert_eq!(class_of("InputEvent").as_deref(), Some("InputEvent"));
    }
}
