//! Builds `timeline.md`, a single human-readable chronological index, and
//! regenerates everything else that is derived from the plain files.
//!
//! Nothing here is a source of truth. Delete `timeline.md`, the search
//! index, `MANIFEST.sha256`, and `README.txt`, and one `timeline build`
//! reconstructs all four from the story files and `archive.yaml`.

use std::fmt;
use std::path::PathBuf;

use crate::cap;
use crate::core::index::rebuild_index;
use crate::core::manifest::write_manifest;
use crate::core::readme;
use crate::core::story::{iter_stories, Story};
use crate::core::vault::Vault;

pub const TIMELINE_FILENAME: &str = "timeline.md";

#[derive(Debug)]
pub struct TimelineError(pub String);

impl fmt::Display for TimelineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for TimelineError {}

/// What a rebuild produced, so the CLI and API can report identically.
#[derive(Debug, Clone)]
pub struct RebuildResult {
    pub story_count: usize,
    pub timeline_path: PathBuf,
    pub index_path: PathBuf,
    pub manifest_path: PathBuf,
    pub readme_path: PathBuf,
}

/// Group stories by year bucket, chronologically, undated last.
fn group_by_year(stories: &[Story]) -> Vec<(String, Vec<&Story>)> {
    let mut ordered: Vec<&Story> = stories.iter().collect();
    ordered.sort_by_key(|story| story.fuzzy_date().sort_key());

    let mut groups: Vec<(String, Vec<&Story>)> = Vec::new();
    for story in ordered {
        let label = match story.fuzzy_date().year() {
            Some(year) => year.to_string(),
            None => "Undated".to_owned(),
        };
        match groups.last_mut() {
            Some((existing, bucket)) if *existing == label => bucket.push(story),
            Some(_) | None => groups.push((label, vec![story])),
        }
    }
    groups
}

pub fn render_timeline(stories: &[Story]) -> String {
    let mut lines = String::from("# Timeline\n\n");
    if stories.is_empty() {
        lines.push_str("(no stories yet)\n");
        return lines;
    }

    for (label, bucket) in group_by_year(stories) {
        lines.push_str(&format!("## {label}\n\n"));
        for story in bucket {
            let date_label = story
                .date
                .clone()
                .unwrap_or_else(|| "date unknown".to_owned());
            let path = story.relative_path();
            let link = path
                .components()
                .filter_map(|component| component.as_os_str().to_str())
                .collect::<Vec<&str>>()
                .join("/");
            lines.push_str(&format!(
                "- **{}** ({date_label}) — [{}]({link})\n",
                story.title, story.id
            ));
        }
        lines.push('\n');
    }
    lines.trim_end().to_owned() + "\n"
}

pub fn build_timeline(vault: &Vault) -> Result<PathBuf, TimelineError> {
    let stories = iter_stories(&vault.root).map_err(|error| TimelineError(error.0))?;
    let path = vault.root.join(TIMELINE_FILENAME);
    cap::fs::write(&path, &render_timeline(&stories))
        .map_err(|error| TimelineError(format!("failed to write timeline.md: {error}")))?;
    Ok(path)
}

/// Regenerate every derived artefact. The manifest is written last so it
/// covers the other three.
pub fn rebuild_derived_state(vault: &Vault) -> Result<RebuildResult, TimelineError> {
    let config = vault
        .load_config()
        .map_err(|error| TimelineError(error.0))?;

    let readme_path = vault.readme_path();
    cap::fs::write(&readme_path, &readme::render(&config))
        .map_err(|error| TimelineError(format!("failed to write README.txt: {error}")))?;

    let stories = iter_stories(&vault.root).map_err(|error| TimelineError(error.0))?;
    let timeline_path = vault.root.join(TIMELINE_FILENAME);
    cap::fs::write(&timeline_path, &render_timeline(&stories))
        .map_err(|error| TimelineError(format!("failed to write timeline.md: {error}")))?;

    let index_path = vault.index_db_path();
    rebuild_index(&vault.root, &index_path).map_err(|error| TimelineError(error.0))?;

    let manifest_path = write_manifest(&vault.root).map_err(|error| TimelineError(error.0))?;

    Ok(RebuildResult {
        story_count: stories.len(),
        timeline_path,
        index_path,
        manifest_path,
        readme_path,
    })
}

#[cfg(test)]
mod tests {
    use super::{rebuild_derived_state, render_timeline};
    use crate::cap;
    use crate::core::manifest::verify_manifest;
    use crate::core::story::{iter_stories, new_story, save_story, NewStory};
    use crate::core::vault::Vault;

    fn seeded() -> (tempfile::TempDir, Vault) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let vault = Vault::init(&temp.path().join("arch"), "Jane Doe", 2, 4).expect("init");
        for (title, date) in [
            ("Starting university", Some("1994-09-15")),
            ("Childhood home", Some("1970s")),
            ("Undated memory", None),
        ] {
            let story = new_story(NewStory {
                title: title.to_owned(),
                date: date.map(str::to_owned),
                body: "Body text.".to_owned(),
                ..NewStory::default()
            })
            .expect("story");
            save_story(&vault.root, &story, false).expect("save");
        }
        (temp, vault)
    }

    #[test]
    fn groups_by_year_chronologically_with_undated_last() {
        let (_temp, vault) = seeded();
        let stories = iter_stories(&vault.root).expect("stories");
        let text = render_timeline(&stories);

        let decade = text.find("## 1970").expect("1970 heading");
        let year = text.find("## 1994").expect("1994 heading");
        let undated = text.find("## Undated").expect("undated heading");
        assert!(decade < year, "1970s sorts before 1994");
        assert!(year < undated, "undated stories come last");

        assert!(text.contains("Starting university"));
        assert!(text.contains("timeline/1994/1994-09-15-starting-university.md"));
    }

    #[test]
    fn an_empty_archive_still_renders() {
        let text = render_timeline(&[]);
        assert!(text.contains("(no stories yet)"));
    }

    #[test]
    fn rebuild_writes_all_four_derived_files() {
        let (_temp, vault) = seeded();
        let result = rebuild_derived_state(&vault).expect("rebuild");

        assert_eq!(result.story_count, 3);
        assert!(cap::fs::exists(&result.timeline_path));
        assert!(cap::fs::exists(&result.index_path));
        assert!(cap::fs::exists(&result.manifest_path));
        assert!(cap::fs::exists(&result.readme_path));
    }

    #[test]
    fn the_manifest_written_by_a_rebuild_verifies_clean() {
        let (_temp, vault) = seeded();
        rebuild_derived_state(&vault).expect("rebuild");
        assert!(verify_manifest(&vault.root).expect("verify").is_empty());
    }

    #[test]
    fn a_rebuild_refreshes_the_readme_from_archive_yaml() {
        let (_temp, vault) = seeded();
        let mut config = vault.load_config().expect("config");
        if let Some(tier) = config.tier_mut("family") {
            tier.identity_sha256 = Some("c".repeat(64));
        }
        vault.save_config(&config).expect("save");

        rebuild_derived_state(&vault).expect("rebuild");
        let readme = cap::fs::read_to_string(&vault.readme_path()).expect("readme");
        assert!(readme.contains(&"c".repeat(64)));
    }

    #[test]
    fn derived_files_can_be_deleted_and_regenerated() {
        let (_temp, vault) = seeded();
        let result = rebuild_derived_state(&vault).expect("rebuild");
        let original = cap::fs::read_to_string(&result.timeline_path).expect("timeline");

        cap::fs::remove_file(&result.timeline_path).expect("delete");
        cap::fs::remove_file(&result.index_path).expect("delete");

        let again = rebuild_derived_state(&vault).expect("rebuild again");
        assert_eq!(
            cap::fs::read_to_string(&again.timeline_path).expect("timeline"),
            original
        );
    }
}
