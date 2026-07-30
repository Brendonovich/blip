use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

pub(crate) type ElementId = u64;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Source {
    Display(u32),
    Window(u32),
    Camera(u64),
    Color { id: u64, value: [u8; 3] },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct Element {
    pub(crate) id: ElementId,
    pub(crate) source: Source,
    pub(crate) center: [f32; 2],
    pub(crate) size: [f32; 2],
    pub(crate) corner_radius_ratio: f32,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct Project {
    pub(crate) elements: Vec<Element>,
    pub(crate) selected_item: Option<ElementId>,
}

impl Project {
    fn is_valid(&self) -> bool {
        let mut ids = HashSet::new();
        self.elements.iter().all(|element| {
            element.id > 0
                && element.id < ElementId::MAX
                && ids.insert(element.id)
                && element.center.into_iter().all(f32::is_finite)
                && element
                    .size
                    .into_iter()
                    .all(|value| value.is_finite() && value > 0.0)
                && element.corner_radius_ratio.is_finite()
                && element.corner_radius_ratio >= 0.0
        }) && self
            .selected_item
            .is_none_or(|selected| ids.contains(&selected))
    }
}

pub(crate) fn load() -> Option<Project> {
    let contents = fs::read_to_string(project_path()).ok()?;
    let project = serde_json::from_str::<Project>(&contents).ok()?;
    project.is_valid().then_some(project)
}

pub(crate) fn save(project: &Project) -> anyhow::Result<()> {
    let path = project_path();
    let parent = path.parent().context("project path has no parent")?;
    fs::create_dir_all(parent).context("failed to create studio settings directory")?;
    let contents = serde_json::to_string_pretty(project)?;
    let temporary_path = path.with_extension("tmp");
    fs::write(&temporary_path, contents).context("failed to write studio project")?;
    fs::rename(temporary_path, path).context("failed to save studio project")
}

fn project_path() -> PathBuf {
    std::env::var_os("HOME")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(Path::new("Library/Application Support/Blip Studio"))
        .join("project.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_round_trips() {
        let project = Project {
            elements: vec![Element {
                id: 4,
                source: Source::Color {
                    id: 2,
                    value: [88, 101, 242],
                },
                center: [0.25, 0.75],
                size: [0.5, 0.5],
                corner_radius_ratio: 0.08,
            }],
            selected_item: Some(4),
        };

        let encoded = serde_json::to_string(&project).expect("encode project");
        let decoded = serde_json::from_str::<Project>(&encoded).expect("decode project");

        assert_eq!(decoded, project);
        assert!(decoded.is_valid());
    }

    #[test]
    fn rejects_invalid_or_duplicate_elements() {
        let element = Element {
            id: 1,
            source: Source::Display(1),
            center: [0.5, 0.5],
            size: [1.0, 1.0],
            corner_radius_ratio: 0.0,
        };
        assert!(
            !Project {
                elements: vec![element, element],
                selected_item: Some(1),
            }
            .is_valid()
        );

        assert!(
            !Project {
                elements: vec![Element {
                    size: [f32::NAN, 1.0],
                    ..element
                }],
                selected_item: Some(2),
            }
            .is_valid()
        );
    }
}
