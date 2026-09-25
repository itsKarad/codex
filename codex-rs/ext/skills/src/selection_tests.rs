use codex_protocol::user_input::UserInput;
use pretty_assertions::assert_eq;

use super::collect_explicit_skill_mentions;
use crate::catalog::SkillAuthority;
use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;
use crate::catalog::SkillPackageId;
use crate::catalog::SkillResourceId;
use crate::catalog::SkillSourceKind;

fn entry(name: &str, id: &str) -> SkillCatalogEntry {
    SkillCatalogEntry::new(
        SkillPackageId(id.to_string()),
        SkillAuthority::new(SkillSourceKind::Host, "host"),
        name,
        format!("Instructions for {name}."),
        SkillResourceId::new(format!("{id}/SKILL.md")),
    )
    .with_display_path(format!("/skills/{id}/SKILL.md"))
}

fn text(value: &str) -> UserInput {
    UserInput::Text {
        text: value.to_string(),
        text_elements: Vec::new(),
    }
}

#[test]
fn resolves_a_unique_short_name_for_a_qualified_skill() {
    let pdf = entry("pdf:pdf", "pdf");
    let catalog = SkillCatalog {
        entries: vec![pdf.clone()],
        warnings: Vec::new(),
    };

    assert_eq!(
        collect_explicit_skill_mentions(&[text("Use $pdf for this task")], &catalog),
        vec![pdf]
    );
}

#[test]
fn leaves_an_ambiguous_short_name_unresolved() {
    let catalog = SkillCatalog {
        entries: vec![entry("pdf:pdf", "pdf"), entry("other:pdf", "other-pdf")],
        warnings: Vec::new(),
    };

    assert_eq!(
        collect_explicit_skill_mentions(&[text("Use $pdf for this task")], &catalog),
        Vec::new()
    );
}
