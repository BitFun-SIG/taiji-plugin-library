//! Native Skill copy provenance and publication, owned alongside registry filesystem IO.
use super::*;
use openbitfun_agent_runtime::skills::SkillImportOrigin;
use sha2::{Digest, Sha256};

pub const IMPORT_MARKER: &str = ".openbitfun-import.json";

pub fn parse_import_origin(content: &str) -> Result<SkillImportOrigin, String> {
    let origin: SkillImportOrigin =
        serde_json::from_str(content).map_err(|_| "Invalid Skill import record")?;
    if origin.schema_version != 1
        || origin.import_id.is_empty()
        || origin.source_key.is_empty()
        || origin.source_id.is_empty()
        || origin.source_path.is_empty()
        || origin.fingerprint.is_empty()
    {
        return Err("Unsupported or incomplete Skill import record".into());
    }
    Ok(origin)
}

pub async fn read_import_origin(path: &Path) -> Result<Option<SkillImportOrigin>, String> {
    use tokio::io::AsyncReadExt;
    let file = match fs::File::open(path.join(IMPORT_MARKER)).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut content = String::new();
    file.take(16 * 1024 + 1)
        .read_to_string(&mut content)
        .await
        .map_err(|error| error.to_string())?;
    if content.len() > 16 * 1024 {
        return Err("Skill import record is too large".into());
    }
    parse_import_origin(&content).map(Some)
}

/// Remove only the reviewed native copy; serialize with publication of new copies.
pub async fn remove_imported_copy(path: &Path, expected_import_id: &str) -> Result<(), String> {
    let root = path.parent().ok_or("Invalid Skill target")?;
    let _lock = openbitfun_services_core::json_store::JsonFileStore
        .acquire_cross_process_lock(&root.join(".skill-import-lock"))
        .await
        .map_err(|error| error.to_string())?;
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| error.to_string())?;
    if is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
        return Err("Imported Skill target changed; refresh before removing".into());
    }
    let origin = read_import_origin(path)
        .await?
        .ok_or("Skill import identity is missing; refresh before removing")?;
    if origin.import_id != expected_import_id {
        return Err("Skill import identity changed; refresh before removing".into());
    }
    fs::remove_dir_all(path)
        .await
        .map_err(|error| error.to_string())
}

// Package traversal never follows links inside a bundle. A linked package root is
// resolved once; copying arbitrary linked files would no longer be a standalone copy.
fn package_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut count = 0;
    while let Some((directory, depth)) = pending.pop() {
        if depth > 32 {
            return Err("Skill package nesting limit exceeded".into());
        }
        for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            count += 1;
            if count > 32768 {
                return Err("Skill package entry limit exceeded".into());
            }
            let metadata =
                std::fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if is_symlink_or_reparse(&metadata) {
                return Err(
                    "Skill package contains linked dependencies; import a standalone copy".into(),
                );
            }
            if metadata.is_dir() {
                pending.push((entry.path(), depth + 1));
            } else if metadata.is_file() {
                files.push(entry.path());
            } else {
                return Err("Skill package contains an unsupported file type".into());
            }
        }
    }
    files.retain(|path| path.file_name().is_none_or(|name| name != IMPORT_MARKER));
    files.sort();
    Ok(files)
}

pub fn package_fingerprint(root: &Path) -> Result<String, String> {
    let mut digest = Sha256::new();
    for path in package_files(root)? {
        let relative = path.strip_prefix(root).map_err(|error| error.to_string())?;
        let relative = relative.to_string_lossy().replace('\\', "/");
        digest.update((relative.len() as u64).to_le_bytes());
        digest.update(relative.as_bytes());
        let mut file = std::fs::File::open(&path).map_err(|error| error.to_string())?;
        digest.update(
            file.metadata()
                .map_err(|error| error.to_string())?
                .len()
                .to_le_bytes(),
        );
        let mut buffer = [0u8; 64 * 1024];
        loop {
            use std::io::Read;
            let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Idempotently imports one discovered external package. Existing distinct user
/// content is never overwritten. Identical legacy copies can acquire provenance.
pub async fn import_copy(
    source: SkillInfo,
    target_root: PathBuf,
) -> Result<SkillImportOrigin, String> {
    import_copy_as(source, target_root, None).await
}

/// A reviewed alias changes both the native directory and invocation name.
/// The source package and every existing native copy remain untouched.
pub async fn import_copy_as(
    source: SkillInfo,
    target_root: PathBuf,
    target_name: Option<String>,
) -> Result<SkillImportOrigin, String> {
    if let Some(name) = &target_name {
        let reserved = name
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if name.is_empty()
            || name.len() > 100
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            || matches!(
                reserved.as_str(),
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "COM1"
                    | "COM2"
                    | "COM3"
                    | "COM4"
                    | "COM5"
                    | "COM6"
                    | "COM7"
                    | "COM8"
                    | "COM9"
                    | "LPT1"
                    | "LPT2"
                    | "LPT3"
                    | "LPT4"
                    | "LPT5"
                    | "LPT6"
                    | "LPT7"
                    | "LPT8"
                    | "LPT9"
            )
        {
            return Err(
                "Invalid Skill import name: use 1–100 letters, digits, hyphens or underscores"
                    .into(),
            );
        }
    }
    if source.is_builtin || source.source_id == OPENBITFUN_SKILL_SOURCE_ID {
        return Err("Expected an external Skill source".into());
    }
    fs::create_dir_all(&target_root)
        .await
        .map_err(|error| error.to_string())?;
    let _lock = openbitfun_services_core::json_store::JsonFileStore
        .acquire_cross_process_lock(&target_root.join(".skill-import-lock"))
        .await
        .map_err(|error| error.to_string())?;
    let result = tokio::task::spawn_blocking(move || {
        use std::fs as disk;
        let source_root = disk::canonicalize(&source.path).map_err(|error| error.to_string())?;
        let folder = target_name.as_ref().unwrap_or(&source.dir_name);
        if folder.is_empty()
            || folder == "."
            || folder == ".."
            || folder.contains(['/', '\\', '\0'])
        {
            return Err("Invalid Skill directory name".into());
        }
        disk::create_dir_all(&target_root).map_err(|error| error.to_string())?;
        let target = target_root.join(folder);
        let staging_root = target_root
            .parent()
            .ok_or("Invalid Skill target root")?
            .join("skill-import-staging");
        disk::create_dir_all(&staging_root).map_err(|error| error.to_string())?;
        let import_id = uuid::Uuid::new_v4().to_string();
        let staging = staging_root.join(&import_id);
        disk::create_dir(&staging).map_err(|error| error.to_string())?;
        let prepared = (|| {
            if let Some(entry) = source
                .entry_file
                .as_deref()
                .filter(|entry| *entry != "SKILL.md")
            {
                if entry.contains(['/', '\\']) {
                    return Err("Invalid Skill entry file".into());
                }
                if is_symlink_or_reparse(
                    &disk::symlink_metadata(source_root.join(entry))
                        .map_err(|error| error.to_string())?,
                ) {
                    return Err("Skill entry is linked; import a standalone copy".into());
                }
                disk::copy(source_root.join(entry), staging.join("SKILL.md"))
                    .map_err(|error| error.to_string())?;
            } else {
                for file in package_files(&source_root)? {
                    let destination = staging.join(
                        file.strip_prefix(&source_root)
                            .map_err(|error| error.to_string())?,
                    );
                    if let Some(parent) = destination.parent() {
                        disk::create_dir_all(parent).map_err(|error| error.to_string())?;
                    }
                    disk::copy(file, destination).map_err(|error| error.to_string())?;
                }
            }
            let mut markdown = disk::read_to_string(staging.join("SKILL.md"))
                .map_err(|error| error.to_string())?;
            if let Some(name) = &target_name {
                let content = markdown.trim_start_matches('\u{feff}');
                let mut lines = content.split_inclusive('\n');
                let first = lines.next().ok_or("Skill frontmatter is missing")?;
                if first.trim() != "---" {
                    return Err("Skill frontmatter is missing".into());
                }
                let mut end = first.len();
                let mut header = String::new();
                let mut closed = false;
                for line in lines {
                    end += line.len();
                    if line.trim() == "---" {
                        closed = true;
                        break;
                    }
                    header.push_str(line);
                }
                if !closed {
                    return Err("Skill frontmatter is incomplete".into());
                }
                let mut metadata: serde_yaml::Mapping =
                    serde_yaml::from_str(&header).map_err(|error| error.to_string())?;
                metadata.insert(
                    serde_yaml::Value::String("name".into()),
                    serde_yaml::Value::String(name.clone()),
                );
                markdown = format!(
                    "---\n{}---\n{}",
                    serde_yaml::to_string(&metadata).map_err(|error| error.to_string())?,
                    &content[end..]
                );
                disk::write(staging.join("SKILL.md"), &markdown)
                    .map_err(|error| error.to_string())?;
            }
            SkillRegistry::parse_skill_markdown(
                target.to_string_lossy().into_owned(),
                &markdown,
                source.level,
                false,
                &source.source_slot,
            )
            .map_err(|error| error.to_string())?;
            let fingerprint = package_fingerprint(&staging)?;
            let origin = SkillImportOrigin {
                schema_version: 1,
                import_id,
                source_key: source.key,
                source_path: source.path,
                source_id: source.source_id,
                source_label: source.source_label,
                source_slot: source.source_slot,
                fingerprint,
            };
            if target.exists() {
                if is_symlink_or_reparse(
                    &disk::symlink_metadata(&target).map_err(|error| error.to_string())?,
                ) {
                    return Err("Skill target is a link; refusing to modify its destination".into());
                }
                let marker = target.join(IMPORT_MARKER);
                if marker.exists() {
                    let existing = parse_import_origin(
                        &disk::read_to_string(&marker).map_err(|error| error.to_string())?,
                    )?;
                    if existing.source_key == origin.source_key
                        && existing.source_path == origin.source_path
                    {
                        return Ok(existing);
                    }
                    return Err("Skill target belongs to a different import".into());
                }
                if package_fingerprint(&target)? != origin.fingerprint {
                    return Err("Skill target already exists with different content".into());
                }
                // A confirmed re-import can repair a byte-identical legacy copy.
                use std::io::Write;
                let mut file = disk::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(marker)
                    .map_err(|error| error.to_string())?;
                file.write_all(
                    &serde_json::to_vec_pretty(&origin).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
                file.sync_all().map_err(|error| error.to_string())?;
                return Ok(origin);
            }
            disk::write(
                staging.join(IMPORT_MARKER),
                serde_json::to_vec_pretty(&origin).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            // Publish the complete package in a single filesystem operation.
            disk::rename(&staging, &target).map_err(|error| error.to_string())?;
            Ok(origin)
        })();
        // This UUID staging directory is created by this operation, never a user input path.
        let _ = disk::remove_dir_all(&staging);
        prepared
    })
    .await
    .map_err(|error| error.to_string())?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(
        path: PathBuf,
        source_id: &'static str,
        slot: &'static str,
        priority: usize,
    ) -> SkillRootEntry {
        SkillRootEntry {
            path,
            level: SkillLocation::User,
            slot,
            source_id,
            source_label: source_id,
            priority,
            is_builtin: false,
        }
    }

    async fn source(temp: &Path) -> SkillInfo {
        let path = temp.join("external/demo");
        fs::create_dir_all(path.join("scripts")).await.unwrap();
        // Claude permits a directory-name fallback; native parsing alone rejects it.
        fs::write(path.join("SKILL.md"), "---\ndescription: Imported workflow\nargument-hint: target\n---\nRun scripts/tool.py for $ARGUMENTS.\n").await.unwrap();
        fs::write(path.join("scripts/tool.py"), "print('fixture')\n")
            .await
            .unwrap();
        SkillRegistry::scan_skills_in_dir(&root(
            temp.join("external"),
            "claude-code",
            "home.claude",
            1,
        ))
        .await
        .candidates
        .remove(0)
        .info
    }

    #[tokio::test]
    async fn imported_package_preserves_dialect_assets_identity_and_runtime_selection() {
        let temp = tempfile::tempdir().unwrap();
        let source = source(temp.path()).await;
        let target = temp.path().join("native");
        let origin = import_copy(source.clone(), target.clone()).await.unwrap();
        let native = SkillRegistry::scan_skills_in_dir(&root(
            target.clone(),
            "openbitfun",
            OPENBITFUN_USER_SKILL_SLOT,
            0,
        ))
        .await
        .candidates
        .remove(0);
        assert_eq!(native.info.source_id, "openbitfun");
        assert_eq!(native.info.import_origin.as_ref().unwrap(), &origin);
        assert_eq!(native.info.argument_hint.as_deref(), Some("target"));
        let content = SkillRegistry::read_local_skill_markdown(&native.info)
            .await
            .unwrap();
        let loaded = SkillRegistry::parse_skill_markdown(
            native.info.path.clone(),
            &content,
            SkillLocation::User,
            true,
            native.info.parser_source_slot(),
        )
        .unwrap();
        assert_eq!(loaded.name, "demo");
        assert!(content.contains("scripts/tool.py"));
        assert!(target.join("demo/scripts/tool.py").is_file());
        let mut external = SkillRegistry::scan_skills_in_dir(&root(
            temp.path().join("external"),
            "claude-code",
            "home.claude",
            1,
        ))
        .await
        .candidates;
        external.push(native);
        let selected = resolve_visible_skills(external);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].source_id, "openbitfun");
        fs::write(target.join("demo/scripts/tool.py"), "user edit")
            .await
            .unwrap();
        assert_eq!(
            import_copy(source, target.clone()).await.unwrap().import_id,
            origin.import_id
        );
        assert_eq!(
            fs::read_to_string(target.join("demo/scripts/tool.py"))
                .await
                .unwrap(),
            "user edit"
        );
    }

    #[tokio::test]
    async fn confirmed_legacy_reimport_adopts_identical_copy_and_rejects_different_content() {
        let temp = tempfile::tempdir().unwrap();
        let source = source(temp.path()).await;
        let target = temp.path().join("native");
        let first = import_copy(source.clone(), target.clone()).await.unwrap();
        fs::remove_file(target.join("demo").join(IMPORT_MARKER))
            .await
            .unwrap();
        let repaired = import_copy(source.clone(), target.clone()).await.unwrap();
        assert_ne!(first.import_id, repaired.import_id);
        assert_eq!(first.fingerprint, repaired.fingerprint);
        fs::remove_file(target.join("demo").join(IMPORT_MARKER))
            .await
            .unwrap();
        fs::write(target.join("demo/SKILL.md"), "user content")
            .await
            .unwrap();
        assert!(import_copy(source, target.clone())
            .await
            .unwrap_err()
            .contains("different content"));
        assert_eq!(
            fs::read_to_string(target.join("demo/SKILL.md"))
                .await
                .unwrap(),
            "user content"
        );
        assert!(!target.join("demo").join(IMPORT_MARKER).exists());
    }

    #[tokio::test]
    async fn flat_pi_entry_imports_only_its_own_markdown_as_native_package() {
        let temp = tempfile::tempdir().unwrap();
        let external = temp.path().join("external");
        fs::create_dir(&external).await.unwrap();
        fs::write(
            external.join("demo.md"),
            "---\ndescription: Flat workflow\n---\nHello\n",
        )
        .await
        .unwrap();
        fs::write(
            external.join("other.md"),
            "---\nname: other\ndescription: Other\n---\nPrivate\n",
        )
        .await
        .unwrap();
        let source = SkillRegistry::scan_skills_in_dir(&root(external, "pi", "home.pi", 1))
            .await
            .candidates
            .into_iter()
            .find(|c| c.info.dir_name == "demo")
            .unwrap()
            .info;
        assert_eq!(source.name, "demo");
        let content = SkillRegistry::read_local_skill_markdown(&source)
            .await
            .unwrap();
        let loaded = SkillRegistry::parse_skill_markdown(
            source.parser_path(),
            &content,
            SkillLocation::User,
            true,
            source.parser_source_slot(),
        )
        .unwrap();
        assert_eq!(loaded.name, "demo");
        let target = temp.path().join("native");
        import_copy(source, target.clone()).await.unwrap();
        assert!(target.join("demo/SKILL.md").is_file());
        assert!(!target.join("demo/other.md").exists());
    }

    #[tokio::test]
    async fn reviewed_alias_keeps_both_skills_callable_and_undo_preserves_originals() {
        let temp = tempfile::tempdir().unwrap();
        let source = source(temp.path()).await;
        let original = fs::read_to_string(Path::new(&source.path).join("SKILL.md"))
            .await
            .unwrap();
        let target = temp.path().join("native");
        import_copy(source.clone(), target.clone()).await.unwrap();
        fs::write(target.join("demo/scripts/tool.py"), "existing user edit")
            .await
            .unwrap();
        let alias = import_copy_as(
            source.clone(),
            target.clone(),
            Some("demo-claude-code".into()),
        )
        .await
        .unwrap();
        let candidates = SkillRegistry::scan_skills_in_dir(&root(
            target.clone(),
            "openbitfun",
            OPENBITFUN_USER_SKILL_SLOT,
            0,
        ))
        .await
        .candidates;
        let resolved = resolve_visible_skills(candidates);
        assert_eq!(resolved.len(), 2);
        let renamed = resolved
            .iter()
            .find(|skill| skill.name == "demo-claude-code")
            .unwrap();
        assert_eq!(renamed.dir_name, "demo-claude-code");
        assert_eq!(
            renamed.import_origin.as_ref().unwrap().source_key,
            source.key
        );
        assert_eq!(renamed.argument_hint.as_deref(), Some("target"));
        assert_eq!(
            fs::read_to_string(Path::new(&source.path).join("SKILL.md"))
                .await
                .unwrap(),
            original
        );
        assert_eq!(
            import_copy_as(
                source.clone(),
                target.clone(),
                Some("demo-claude-code".into())
            )
            .await
            .unwrap()
            .import_id,
            alias.import_id
        );
        remove_imported_copy(&target.join("demo-claude-code"), &alias.import_id)
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(target.join("demo/scripts/tool.py"))
                .await
                .unwrap(),
            "existing user edit"
        );
        assert!(Path::new(&source.path).join("SKILL.md").is_file());
        for name in ["../escape", "CON", "bad/name", "", "name."] {
            assert!(
                import_copy_as(source.clone(), target.clone(), Some(name.into()))
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn undo_keeps_source_and_rejects_a_receipt_for_a_replaced_copy() {
        let temp = tempfile::tempdir().unwrap();
        let source = source(temp.path()).await;
        let target = temp.path().join("native");
        let first = import_copy(source.clone(), target.clone()).await.unwrap();
        remove_imported_copy(&target.join("demo"), &first.import_id)
            .await
            .unwrap();
        assert!(Path::new(&source.path).join("SKILL.md").is_file());
        let second = import_copy(source, target.clone()).await.unwrap();
        assert!(remove_imported_copy(&target.join("demo"), &first.import_id)
            .await
            .is_err());
        assert_eq!(
            read_import_origin(&target.join("demo"))
                .await
                .unwrap()
                .unwrap()
                .import_id,
            second.import_id
        );
    }
}
