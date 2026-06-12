//! Collection search over skill and agent frontmatter.

use anyhow::{Context, Result};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use serde::Serialize;
use std::cmp::Ordering;
use std::io::ErrorKind;
use std::path::Path;

use crate::collection::Collection;
use crate::lockfile::ItemType;

/// One ranked collection search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub name: String,
    pub description: String,
    pub category: String,
    pub score: i64,
}

#[derive(Debug, Default)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    category: Option<String>,
}

/// Search collection skills and agents by frontmatter name and description.
///
/// An empty query returns every item with score `0`.
pub fn search(collection: &Collection, query: &str) -> Result<Vec<SearchHit>> {
    let query = query.trim();
    let mut items = scan_items(collection)?;
    let matcher = SkimMatcherV2::default();

    let mut hits = Vec::new();
    for mut item in items.drain(..) {
        if let Some(score) = match_score(&matcher, query, &item) {
            item.score = score;
            hits.push(item);
        }
    }

    hits.sort_by(compare_hits);
    Ok(hits)
}

fn scan_items(collection: &Collection) -> Result<Vec<SearchHit>> {
    let mut items = Vec::new();
    scan_skills(collection, &mut items)?;
    scan_agents(collection, &mut items)?;
    Ok(items)
}

fn scan_skills(collection: &Collection, items: &mut Vec<SearchHit>) -> Result<()> {
    let skills_dir = collection.root.join("skills");
    let entries = match std::fs::read_dir(&skills_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", skills_dir.display())),
    };

    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", skills_dir.display()))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let fallback_name = entry.file_name().to_string_lossy().into_owned();
        items.push(hit_from_file(ItemType::Skill, &fallback_name, &skill_md));
    }
    Ok(())
}

fn scan_agents(collection: &Collection, items: &mut Vec<SearchHit>) -> Result<()> {
    let agents_dir = collection.root.join("agents");
    let entries = match std::fs::read_dir(&agents_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", agents_dir.display())),
    };

    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", agents_dir.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let Some(fallback_name) = file_name.strip_suffix(".agent.md") else {
            continue;
        };
        items.push(hit_from_file(ItemType::Agent, fallback_name, &path));
    }
    Ok(())
}

fn hit_from_file(item_type: ItemType, fallback_name: &str, path: &Path) -> SearchHit {
    let frontmatter = read_frontmatter(path);
    SearchHit {
        item_type,
        name: frontmatter
            .name
            .unwrap_or_else(|| fallback_name.to_string()),
        description: frontmatter.description.unwrap_or_default(),
        category: frontmatter.category.unwrap_or_default(),
        score: 0,
    }
}

fn read_frontmatter(path: &Path) -> Frontmatter {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_frontmatter(path, &content),
        Err(e) => {
            eprintln!(
                "warning: could not read frontmatter from {}: {e}",
                path.display()
            );
            Frontmatter::default()
        }
    }
}

fn parse_frontmatter(path: &Path, content: &str) -> Frontmatter {
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        eprintln!(
            "warning: {} has no frontmatter; using filename defaults",
            path.display()
        );
        return Frontmatter::default();
    }

    let mut frontmatter = Frontmatter::default();
    let mut found_end = false;
    for (index, raw_line) in lines.enumerate() {
        let line = raw_line.trim();
        if line == "---" {
            found_end = true;
            break;
        }
        parse_frontmatter_line(path, index + 2, line, &mut frontmatter);
    }

    if !found_end {
        eprintln!(
            "warning: {} has malformed frontmatter (missing closing ---); using filename defaults",
            path.display()
        );
        return Frontmatter::default();
    }

    frontmatter
}

fn parse_frontmatter_line(
    path: &Path,
    line_number: usize,
    line: &str,
    frontmatter: &mut Frontmatter,
) {
    if line.is_empty() || line.starts_with('#') {
        return;
    }

    let Some((key, value)) = line.split_once(':') else {
        eprintln!(
            "warning: {}:{line_number} has malformed frontmatter field; skipping",
            path.display()
        );
        return;
    };

    let key = key.trim();
    let value = value.trim();
    match key {
        "name" => frontmatter.name = parse_scalar(path, line_number, key, value),
        "description" => frontmatter.description = parse_scalar(path, line_number, key, value),
        "category" => frontmatter.category = parse_scalar(path, line_number, key, value),
        _ => {}
    }
}

fn parse_scalar(path: &Path, line_number: usize, key: &str, value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }

    let quoted = (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''));
    let unbalanced_quote = value.starts_with('"') != value.ends_with('"')
        || value.starts_with('\'') != value.ends_with('\'');

    if unbalanced_quote {
        eprintln!(
            "warning: {}:{line_number} has malformed {key} field; skipping",
            path.display()
        );
        return None;
    }

    let value = if quoted {
        &value[1..value.len() - 1]
    } else {
        value
    };
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn match_score(matcher: &SkimMatcherV2, query: &str, item: &SearchHit) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }

    let name_score = matcher
        .fuzzy_match(&item.name, query)
        .map(|score| score + 10_000);
    let description_score = matcher.fuzzy_match(&item.description, query);

    name_score.into_iter().chain(description_score).max()
}

fn compare_hits(a: &SearchHit, b: &SearchHit) -> Ordering {
    b.score
        .cmp(&a.score)
        .then_with(|| type_rank(a.item_type).cmp(&type_rank(b.item_type)))
        .then_with(|| a.name.cmp(&b.name))
}

fn type_rank(item_type: ItemType) -> u8 {
    match item_type {
        ItemType::Skill => 0,
        ItemType::Agent => 1,
    }
}
