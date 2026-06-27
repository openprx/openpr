pub mod capabilities;
pub mod comments;
pub mod connectors;
pub mod context;
pub mod files;
pub mod forms;
pub mod labels;
pub mod members;
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
    vec![
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
        connectors::list_connectors_tool(),
        connectors::get_connector_tool(),
        connectors::list_invocations_tool(),
        connectors::get_invocation_tool(),
        connectors::create_invocation_tool(),
        connectors::report_invocation_progress_tool(),
        connectors::complete_invocation_tool(),
        connectors::fail_invocation_tool(),
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
    ]
}

#[cfg(test)]
mod tests {
    use super::get_all_tool_definitions;
    use std::collections::HashSet;

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
            "connectors.list",
            "connectors.get",
            "invocations.list",
            "invocations.get",
            "invocations.create",
            "invocations.report_progress",
            "invocations.complete",
            "invocations.fail",
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
            "code.resources.list",
            "code.directory.get",
            "code.task_context.get",
            "code.change_proposal.create",
            "documents.extract_summary",
            "documents.review_risk",
            "approval.request",
            "inspection.report",
            "corrective_action.propose",
        ] {
            assert!(
                unique.contains(expected),
                "missing Phase 1 MCP tool registration: {expected}"
            );
        }
        assert_eq!(
            tools.len(),
            105,
            "Universal forms and plugins slices should expose 105 MCP tools"
        );
    }
}
