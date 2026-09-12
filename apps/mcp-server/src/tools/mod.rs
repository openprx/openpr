pub mod capabilities;
pub mod comments;
pub mod context;
pub mod files;
pub mod flow_features;
pub mod forms;
pub mod labels;
pub mod legacy_pages;
pub mod members;
pub mod objects;
pub mod operation_logs;
pub mod plugins;
pub mod project_types;
pub mod projects;
pub mod proposals;
pub mod release;
pub mod scenario_templates;
pub mod scenario_tools;
pub mod search;
pub mod sprints;
pub mod work_items;

use crate::protocol::ToolDefinition;

pub fn get_all_tool_definitions() -> Vec<ToolDefinition> {
    let mut tools = vec![
        projects::list_projects_tool(),
        projects::get_project_tool(),
        projects::create_project_tool(),
        projects::update_project_tool(),
        projects::delete_project_tool(),
        project_types::list_project_types_tool(),
        project_types::get_project_type_tool(),
        scenario_templates::list_scenario_templates_tool(),
        scenario_templates::get_scenario_template_tool(),
        scenario_templates::install_scenario_template_tool(),
        project_types::list_project_resources_tool(),
        project_types::create_project_resource_tool(),
        project_types::update_project_resource_tool(),
        project_types::delete_project_resource_tool(),
        forms::list_forms_tool(),
        forms::get_form_tool(),
        forms::create_form_tool(),
        forms::create_form_from_template_tool(),
        forms::update_form_schema_tool(),
        forms::duplicate_form_tool(),
        forms::get_form_schema_summary_tool(),
        forms::get_form_field_usage_tool(),
        forms::get_form_field_dependencies_tool(),
        forms::list_form_schema_versions_tool(),
        forms::get_form_schema_version_tool(),
        forms::get_form_permissions_tool(),
        forms::update_form_permissions_tool(),
        forms::list_form_views_tool(),
        forms::list_form_attachments_tool(),
        forms::create_form_attachment_tool(),
        forms::archive_form_attachment_tool(),
        forms::restore_form_attachment_tool(),
        forms::list_form_records_tool(),
        forms::export_form_records_tool(),
        forms::preview_import_form_records_tool(),
        forms::import_form_records_tool(),
        forms::get_form_record_tool(),
        forms::create_form_record_tool(),
        forms::update_form_record_tool(),
        forms::link_form_record_tool(),
        forms::list_form_relation_targets_tool(),
        forms::list_form_record_children_tool(),
        forms::create_child_form_record_tool(),
        forms::update_child_form_record_tool(),
        forms::archive_child_form_record_tool(),
        forms::restore_child_form_record_tool(),
        forms::aggregate_form_records_tool(),
        forms::events_tail_tool(),
        plugins::list_plugins_tool(),
        plugins::get_plugin_tool(),
        plugins::install_plugin_tool(),
        plugins::invoke_plugin_tool(),
        plugins::list_plugin_invocations_tool(),
        context::get_project_context_tool(),
        context::get_governance_context_tool(),
        context::get_agent_policy_tool(),
        work_items::list_work_items_tool(),
        work_items::get_work_item_tool(),
        work_items::get_work_item_by_identifier_tool(),
        work_items::create_work_item_tool(),
        work_items::update_work_item_tool(),
        work_items::add_label_to_work_item_tool(),
        work_items::remove_label_from_work_item_tool(),
        work_items::list_work_item_labels_tool(),
        work_items::delete_work_item_tool(),
        work_items::search_work_items_tool(),
        comments::list_comments_tool(),
        comments::create_comment_tool(),
        comments::delete_comment_tool(),
        files::upload_file_tool(),
        proposals::list_proposals_tool(),
        proposals::get_proposal_tool(),
        proposals::create_proposal_tool(),
        proposals::create_proposal_from_result_tool(),
        proposals::create_check_result_tool(),
        release::get_release_readiness_tool(),
        members::list_members_tool(),
        operation_logs::list_bot_operation_logs_tool(),
        sprints::create_sprint_tool(),
        sprints::list_sprints_tool(),
        sprints::update_sprint_tool(),
        sprints::delete_sprint_tool(),
        labels::create_label_tool(),
        labels::list_labels_tool(),
        labels::list_project_labels_tool(),
        labels::update_label_tool(),
        labels::delete_label_tool(),
        work_items::add_labels_to_work_item_tool(),
        search::search_all_tool(),
        scenario_tools::code_resources_list_tool(),
        scenario_tools::code_directory_get_tool(),
        scenario_tools::code_task_context_get_tool(),
        scenario_tools::code_change_proposal_create_tool(),
        scenario_tools::documents_extract_summary_tool(),
        scenario_tools::documents_review_risk_tool(),
        scenario_tools::approval_request_tool(),
        scenario_tools::inspection_report_tool(),
        scenario_tools::corrective_action_propose_tool(),
        flow_features::get_flow_feature_tool(),
        flow_features::set_flow_feature_tool(),
        objects::get_flow_object_tool(),
        objects::query_flow_objects_tool(),
        objects::get_flow_object_history_tool(),
    ];
    tools.extend(flow_v05_tool_definitions());
    tools.extend(flow_v06_tool_definitions());
    tools.extend([
        legacy_pages::legacy_pages_inventory_tool(),
        legacy_pages::legacy_pages_import_preview_tool(),
        legacy_pages::legacy_pages_import_commit_tool(),
        legacy_pages::legacy_pages_import_status_tool(),
    ]);
    tools
}

fn flow_v06_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        objects::describe_collection_tool(),
        objects::query_collection_tool(),
        objects::create_collection_record_tool(),
    ]
}

/// The live v0.5 registration set. Keeping the version boundary explicit lets the contract test
/// compare both directions without inferring a version from tool ordering or from the snapshot it
/// is meant to verify.
fn flow_v05_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        objects::create_flow_object_tool(),
        objects::patch_flow_object_tool(),
        objects::move_flow_object_tool(),
        objects::link_flow_objects_tool(),
        objects::unlink_flow_objects_tool(),
        objects::diff_flow_object_tool(),
        objects::get_flow_object_grants_tool(),
        objects::set_flow_object_grants_tool(),
        objects::set_flow_object_inheritance_tool(),
        objects::list_flow_object_relations_tool(),
        objects::search_flow_objects_tool(),
        objects::get_flow_projection_lag_tool(),
    ]
}

#[cfg(test)]
mod tests {
    use super::{flow_v05_tool_definitions, flow_v06_tool_definitions, get_all_tool_definitions};
    use sha2::{Digest, Sha256};
    use std::collections::HashSet;

    const FLOW_V05_SURFACE_SNAPSHOT: &str = include_str!("mcp-surface-v05.snapshot.md");
    const TOOL_REGISTRY_BASELINE: &str = include_str!("../../tool-registry-baseline.json");

    #[test]
    fn flow_v06_tools_match_the_repository_registry_baseline() {
        let tools = get_all_tool_definitions();
        let names = tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>();
        let unique = names.iter().copied().collect::<HashSet<_>>();
        let baseline: serde_json::Value =
            serde_json::from_str(TOOL_REGISTRY_BASELINE).expect("tool registry baseline is valid JSON");
        let expected_count = baseline
            .get("count")
            .and_then(serde_json::Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .expect("tool registry baseline count fits usize");
        let expected_hash = baseline
            .get("names_sha256")
            .and_then(serde_json::Value::as_str)
            .expect("tool registry baseline names_sha256 is a string");
        let mut sorted_names = names.clone();
        sorted_names.sort_unstable();
        let names_hash = format!("{:x}", Sha256::digest(sorted_names.join("\n").as_bytes()));
        assert_eq!(
            tools.len(),
            expected_count,
            "live registry count must match the repository-owned baseline"
        );
        assert_eq!(
            names_hash, expected_hash,
            "live registry names must match the baseline hash"
        );
        assert_eq!(names.len(), unique.len(), "MCP tool names must remain unique");

        let live_v06 = flow_v06_tool_definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<HashSet<_>>();
        assert_eq!(
            live_v06,
            HashSet::from([
                "collections.describe".to_string(),
                "collections.query".to_string(),
                "records.create".to_string(),
            ]),
            "the live v0.6 delta must be the frozen three-tool surface"
        );
    }

    #[derive(Debug, PartialEq, Eq)]
    struct FlowV05Surface {
        baseline_total: usize,
        expected_total: usize,
        tool_names: HashSet<String>,
    }

    fn registry_total(surface: &str, version: &str) -> Option<usize> {
        surface.lines().find_map(|line| {
            let marker = format!("v{version} `");
            let tail = line.split_once(&marker)?.1;
            tail.split_once('`')?.0.parse::<usize>().ok()
        })
    }

    fn parse_flow_v05_surface(surface: &str) -> FlowV05Surface {
        let tool_names = surface
            .lines()
            .skip_while(|line| *line != "## Tools")
            .skip(1)
            .take_while(|line| !line.starts_with("## "))
            .filter_map(|line| {
                let cells = line.split('|').map(str::trim).collect::<Vec<_>>();
                match (cells.get(1), cells.get(2)) {
                    (Some(name), Some(&"0.5")) => Some(name.trim_matches('`').to_string()),
                    _ => None,
                }
            })
            .collect::<HashSet<_>>();
        FlowV05Surface {
            baseline_total: registry_total(surface, "0.4")
                .expect("the surface must declare the v0.4 registry baseline"),
            expected_total: registry_total(surface, "0.5").expect("the surface must declare the v0.5 registry total"),
            tool_names,
        }
    }

    #[test]
    fn project_type_and_resource_tools_are_registered_once() {
        let tools = get_all_tool_definitions();
        let names = tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>();
        let unique = names.iter().copied().collect::<HashSet<_>>();

        assert_eq!(names.len(), unique.len(), "MCP tool names must be unique");
        for expected in [
            "project_types.list",
            "project_types.get",
            "scenario_templates.list",
            "scenario_templates.get",
            "scenario_templates.install",
            "project_resources.list",
            "project_resources.create",
            "project_resources.update",
            "project_resources.delete",
            "forms.list",
            "forms.get",
            "forms.create",
            "forms.create_from_template",
            "forms.update_schema",
            "forms.duplicate",
            "forms.schema_summary",
            "forms.field_usage",
            "forms.field_dependencies",
            "form_schema_versions.list",
            "form_schema_versions.get",
            "form_permissions.get",
            "form_permissions.update",
            "form_views.list",
            "form_attachments.list",
            "form_attachments.create",
            "form_attachments.archive",
            "form_attachments.restore",
            "form_records.list",
            "form_records.export",
            "form_records.import_preview",
            "form_records.import_commit",
            "form_records.get",
            "form_records.create",
            "form_records.update",
            "form_records.link",
            "form_records.relation_targets",
            "form_records.children",
            "form_records.child_create",
            "form_records.child_update",
            "form_records.child_archive",
            "form_records.child_restore",
            "form_records.aggregate",
            "events.tail",
            "plugins.list",
            "plugins.get",
            "plugins.install",
            "plugins.invoke",
            "plugin_invocations.list",
            "context.get_project",
            "context.get_governance",
            "context.get_agent_policy",
            "proposals.create_from_result",
            "check_results.create",
            "release.readiness.get",
            "bot_operation_logs.list",
            "code.resources.list",
            "code.directory.get",
            "code.task_context.get",
            "code.change_proposal.create",
            "documents.extract_summary",
            "documents.review_risk",
            "approval.request",
            "inspection.report",
            "corrective_action.propose",
            "flow.feature_get",
            "flow.feature_set",
            "objects.get",
            "objects.query",
            "objects.history",
            "objects.create",
            "objects.patch",
            "objects.move",
            "objects.link",
            "objects.unlink",
            "objects.diff",
            "objects.grants_get",
            "objects.grants_set",
            "objects.inheritance_set",
            "objects.relations",
            "objects.search",
            "collab.projection_lag",
            "legacy_pages.inventory",
            "legacy_pages.import_preview",
            "legacy_pages.import_commit",
            "legacy_pages.import_status",
        ] {
            assert!(
                unique.contains(expected),
                "missing Phase 1 MCP tool registration: {expected}"
            );
        }
    }

    #[test]
    fn flow_v05_registry_matches_the_embedded_contract_snapshot() {
        let tools = get_all_tool_definitions();
        let snapshot = parse_flow_v05_surface(FLOW_V05_SURFACE_SNAPSHOT);
        let expected_v05_count = snapshot
            .expected_total
            .checked_sub(snapshot.baseline_total)
            .expect("the v0.5 total must not be below the v0.4 baseline");
        assert_ne!(expected_v05_count, 0, "the v0.5 delta must not be empty");
        assert_eq!(
            snapshot.tool_names.len(),
            expected_v05_count,
            "the snapshot must contain exactly total(v0.5)-total(v0.4) tool rows"
        );
        // The snapshot freezes the v0.5 delta, not the all-releases live total. Later releases
        // deliberately append tools; their exact current total has its own release test.

        // The live side is independently enumerable from the registration function used by the
        // server. Comparing only `snapshot.iter().all(live.contains)` would make an empty or
        // incomplete parsed table vacuously pass.
        let live_v05 = flow_v05_tool_definitions()
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<HashSet<_>>();
        let registered = tools.iter().map(|tool| tool.name.as_str()).collect::<HashSet<_>>();
        assert_eq!(
            live_v05.len(),
            expected_v05_count,
            "the live v0.5 registry suffix must contain the declared delta"
        );
        assert!(
            snapshot
                .tool_names
                .iter()
                .all(|name| registered.contains(name.as_str())),
            "every embedded v0.5 contract tool must exist in the live registry"
        );
        assert!(
            live_v05.iter().all(|name| snapshot.tool_names.contains(name)),
            "every live v0.5 tool must exist in the embedded contract snapshot"
        );
    }

    #[test]
    #[ignore = "manual contract-drift check; set SYLVODE_FLOW_CONTRACTS_ROOT explicitly"]
    fn flow_v05_embedded_snapshot_matches_the_authoritative_contract() {
        let contract_root = std::env::var("SYLVODE_FLOW_CONTRACTS_ROOT")
            .expect("set SYLVODE_FLOW_CONTRACTS_ROOT to an explicit Sylvode Flow checkout");
        let surface = std::fs::read_to_string(format!("{contract_root}/contracts/mcp-surface-v1.md"))
            .expect("the explicitly selected authoritative MCP surface contract must be readable");
        assert_eq!(
            parse_flow_v05_surface(FLOW_V05_SURFACE_SNAPSHOT),
            parse_flow_v05_surface(&surface),
            "refresh the embedded snapshot after an approved contract change"
        );
    }

    #[test]
    fn flow_v05_write_and_visibility_schemas_preserve_contract_boundaries() {
        let tools = get_all_tool_definitions();
        let by_name = tools
            .iter()
            .map(|tool| (tool.name.as_str(), &tool.input_schema))
            .collect::<std::collections::HashMap<_, _>>();

        for name in [
            "objects.create",
            "objects.patch",
            "objects.move",
            "objects.link",
            "objects.unlink",
            "objects.grants_set",
            "objects.inheritance_set",
        ] {
            let schema = by_name.get(name).unwrap_or_else(|| panic!("missing {name}"));
            let required = schema["required"]
                .as_array()
                .unwrap_or_else(|| panic!("{name} must declare required fields"));
            assert!(
                required.iter().any(|field| field == "idempotency_key"),
                "{name} must require idempotency_key"
            );
            assert!(
                schema["properties"].get("message").is_some(),
                "{name} must accept an optional message"
            );
            assert!(
                !required.iter().any(|field| field == "message"),
                "{name} message must remain optional"
            );
        }

        let move_schema = by_name.get("objects.move").expect("missing objects.move");
        assert!(
            move_schema["required"]
                .as_array()
                .is_some_and(|required| { required.iter().any(|field| field == "target_object_id") })
        );
        assert!(move_schema["properties"].get("expected_target_frontier").is_some());
        assert!(move_schema["properties"].get("expected_frontier").is_none());

        let grants = by_name.get("objects.grants_set").expect("missing objects.grants_set");
        assert!(grants["properties"].get("dry_run").is_some());
        assert!(!by_name.contains_key("objects.grants_set_dry_run"));

        let search = by_name.get("objects.search").expect("missing objects.search");
        assert!(search["properties"].get("all_visible").is_none());

        let projection_lag = by_name
            .get("collab.projection_lag")
            .expect("missing collab.projection_lag");
        for forbidden in ["content", "bytes"] {
            assert!(
                projection_lag["properties"].get(forbidden).is_none(),
                "projection lag input must not expose {forbidden}"
            );
        }
    }
}
