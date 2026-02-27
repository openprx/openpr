pub(crate) mod comments;
pub(crate) mod files;
pub(crate) mod labels;
pub(crate) mod members;
pub(crate) mod projects;
pub(crate) mod proposals;
pub(crate) mod search;
pub(crate) mod sprints;
pub(crate) mod work_items;

use crate::protocol::ToolDefinition;

pub fn get_all_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        projects::list_projects_tool(),
        projects::get_project_tool(),
        projects::create_project_tool(),
        projects::update_project_tool(),
        projects::delete_project_tool(),
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
    ]
}
