use reqwest::{Client, multipart};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct OpenPrClient {
    client: Client,
    base_url: String,
    bot_token: String,
    pub workspace_id: String,
    invocation_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UploadedFile {
    pub url: String,
    pub filename: String,
    pub storage_backend: Option<String>,
    pub object_key: Option<String>,
    pub thumbnail_url: Option<String>,
    pub thumbnail_object_key: Option<String>,
}

impl OpenPrClient {
    pub fn new(base_url: String, bot_token: String, workspace_id: String) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {e}"))?;
        Ok(Self {
            client,
            base_url,
            bot_token,
            workspace_id,
            invocation_id: std::env::var("OPENPR_INVOCATION_ID")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        })
    }

    pub fn invocation_id(&self) -> Option<&str> {
        self.invocation_id.as_deref()
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {status} from {path}: {body}"));
        }

        resp.json::<T>()
            .await
            .map_err(|e| format!("Failed to deserialize response: {e}"))
    }

    pub async fn post<T: DeserializeOwned, B: Serialize + Sync>(&self, path: &str, body: &B) -> Result<T, String> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {status} from {path}: {body}"));
        }

        resp.json::<T>()
            .await
            .map_err(|e| format!("Failed to deserialize response: {e}"))
    }

    pub async fn patch<T: DeserializeOwned, B: Serialize + Sync>(&self, path: &str, body: &B) -> Result<T, String> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .patch(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {status} from {path}: {body}"));
        }

        resp.json::<T>()
            .await
            .map_err(|e| format!("Failed to deserialize response: {e}"))
    }

    pub async fn put<T: DeserializeOwned, B: Serialize + Sync>(&self, path: &str, body: &B) -> Result<T, String> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {status} from {path}: {body}"));
        }

        resp.json::<T>()
            .await
            .map_err(|e| format!("Failed to deserialize response: {e}"))
    }

    pub async fn delete(&self, path: &str) -> Result<(), String> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {status} from {path}: {body}"));
        }

        Ok(())
    }

    // ---- Projects ----

    pub async fn list_projects(&self) -> Result<Value, String> {
        self.get(&format!("/api/v1/workspaces/{}/projects", self.workspace_id))
            .await
    }

    pub async fn get_project(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{project_id}")).await
    }

    pub async fn create_project(&self, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/workspaces/{}/projects", self.workspace_id), &body)
            .await
    }

    pub async fn update_project(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.patch(&format!("/api/v1/projects/{project_id}"), &body).await
    }

    pub async fn delete_project(&self, project_id: &str) -> Result<(), String> {
        self.delete(&format!("/api/v1/projects/{project_id}")).await
    }

    // ---- Project Types / Resources ----

    pub async fn list_project_types(&self) -> Result<Value, String> {
        self.get(&format!("/api/v1/workspaces/{}/project-types", self.workspace_id))
            .await
    }

    pub async fn get_project_type(&self, key: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/project-types/{}", urlencoding::encode(key)))
            .await
    }

    pub async fn list_scenario_templates(
        &self,
        project_type_key: Option<&str>,
        industry: Option<&str>,
    ) -> Result<Value, String> {
        let mut path = "/api/v1/scenario-templates".to_string();
        let mut params = Vec::new();
        if let Some(project_type_key) = project_type_key {
            params.push(format!("project_type_key={}", urlencoding::encode(project_type_key)));
        }
        if let Some(industry) = industry {
            params.push(format!("industry={}", urlencoding::encode(industry)));
        }
        if !params.is_empty() {
            path.push('?');
            path.push_str(&params.join("&"));
        }
        self.get(&path).await
    }

    pub async fn get_scenario_template(&self, key: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/scenario-templates/{}", urlencoding::encode(key)))
            .await
    }

    pub async fn install_scenario_template(&self, project_id: &str, template_key: &str) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v1/projects/{project_id}/scenario-templates/{}/install",
                urlencoding::encode(template_key)
            ),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn list_project_resources(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{project_id}/resources")).await
    }

    pub async fn create_project_resource(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/projects/{project_id}/resources"), &body)
            .await
    }

    pub async fn update_project_resource(
        &self,
        project_id: &str,
        resource_id: &str,
        body: Value,
    ) -> Result<Value, String> {
        self.patch(&format!("/api/v1/projects/{project_id}/resources/{resource_id}"), &body)
            .await
    }

    pub async fn delete_project_resource(&self, project_id: &str, resource_id: &str) -> Result<(), String> {
        self.delete(&format!("/api/v1/projects/{project_id}/resources/{resource_id}"))
            .await
    }

    // ---- Connectors / Invocations ----

    pub async fn list_connectors(&self, project_id: Option<&str>) -> Result<Value, String> {
        let path = project_id.map_or_else(
            || format!("/api/v1/workspaces/{}/connectors", self.workspace_id),
            |project_id| {
                format!(
                    "/api/v1/workspaces/{}/connectors?project_id={}",
                    self.workspace_id,
                    urlencoding::encode(project_id)
                )
            },
        );
        self.get(&path).await
    }

    pub async fn get_connector(&self, connector_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/workspaces/{}/connectors/{}",
            self.workspace_id,
            urlencoding::encode(connector_id)
        ))
        .await
    }

    pub async fn list_invocations(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{project_id}/invocations")).await
    }

    pub async fn get_invocation(&self, invocation_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/invocations/{}", urlencoding::encode(invocation_id)))
            .await
    }

    pub async fn create_invocation(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/projects/{project_id}/invocations"), &body)
            .await
    }

    pub async fn report_invocation_progress(&self, invocation_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/invocations/{}/progress", urlencoding::encode(invocation_id)),
            &body,
        )
        .await
    }

    pub async fn complete_invocation(&self, invocation_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/invocations/{}/complete", urlencoding::encode(invocation_id)),
            &body,
        )
        .await
    }

    pub async fn fail_invocation(&self, invocation_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/invocations/{}/fail", urlencoding::encode(invocation_id)),
            &body,
        )
        .await
    }

    pub async fn report_invocation_tool_call(&self, invocation_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/invocations/{}/tool-calls", urlencoding::encode(invocation_id)),
            &body,
        )
        .await
    }

    // ---- Universal Forms ----

    pub async fn list_forms(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{project_id}/forms")).await
    }

    pub async fn get_form(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/forms/{}", urlencoding::encode(form_id)))
            .await
    }

    pub async fn create_form(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/projects/{project_id}/forms"), &body).await
    }

    pub async fn create_form_from_template(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/projects/{project_id}/forms/from-template"), &body)
            .await
    }

    pub async fn update_form_schema(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.patch(&format!("/api/v1/forms/{}", urlencoding::encode(form_id)), &body)
            .await
    }

    pub async fn duplicate_form(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/forms/{}/duplicate", urlencoding::encode(form_id)),
            &body,
        )
        .await
    }

    pub async fn get_form_schema_summary(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/forms/{}/schema-summary",
            urlencoding::encode(form_id)
        ))
        .await
    }

    pub async fn get_form_field_usage(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/forms/{}/field-usage", urlencoding::encode(form_id)))
            .await
    }

    pub async fn get_form_field_dependencies(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/forms/{}/field-dependencies",
            urlencoding::encode(form_id)
        ))
        .await
    }

    pub async fn list_form_schema_versions(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/forms/{}/schema-versions",
            urlencoding::encode(form_id)
        ))
        .await
    }

    pub async fn get_form_schema_version(&self, form_id: &str, version: i32) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/forms/{}/schema-versions/{version}",
            urlencoding::encode(form_id)
        ))
        .await
    }

    pub async fn get_form_permissions(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/forms/{}/permissions", urlencoding::encode(form_id)))
            .await
    }

    pub async fn update_form_permissions(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.patch(
            &format!("/api/v1/forms/{}/permissions", urlencoding::encode(form_id)),
            &body,
        )
        .await
    }

    pub async fn list_form_views(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/forms/{}/views", urlencoding::encode(form_id)))
            .await
    }

    pub async fn list_form_attachments(
        &self,
        form_id: &str,
        record_id: Option<&str>,
        field_key: Option<&str>,
        include_archived: Option<bool>,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<Value, String> {
        let mut query = Vec::new();
        if let Some(record_id) = record_id {
            query.push(format!("record_id={}", urlencoding::encode(record_id)));
        }
        if let Some(field_key) = field_key {
            query.push(format!("field_key={}", urlencoding::encode(field_key)));
        }
        if let Some(include_archived) = include_archived {
            query.push(format!("include_archived={include_archived}"));
        }
        if let Some(page) = page {
            query.push(format!("page={page}"));
        }
        if let Some(per_page) = per_page {
            query.push(format!("per_page={per_page}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.get(&format!(
            "/api/v1/forms/{}/attachments{}",
            urlencoding::encode(form_id),
            query
        ))
        .await
    }

    pub async fn create_form_attachment(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/forms/{}/attachments", urlencoding::encode(form_id)),
            &body,
        )
        .await
    }

    pub async fn archive_form_attachment(&self, attachment_id: &str) -> Result<(), String> {
        self.delete(&format!(
            "/api/v1/form-attachments/{}",
            urlencoding::encode(attachment_id)
        ))
        .await
    }

    pub async fn restore_form_attachment(&self, attachment_id: &str) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v1/form-attachments/{}/restore",
                urlencoding::encode(attachment_id)
            ),
            &json!({}),
        )
        .await
    }

    pub async fn list_form_records(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/forms/{}/records", urlencoding::encode(form_id)))
            .await
    }

    pub async fn export_form_records(
        &self,
        form_id: &str,
        view_id: Option<&str>,
        columns: Option<&str>,
        format: Option<&str>,
        scope: Option<&str>,
        include_archived: Option<bool>,
        limit: Option<i64>,
    ) -> Result<Value, String> {
        let mut query = Vec::new();
        if let Some(view_id) = view_id {
            query.push(format!("view_id={}", urlencoding::encode(view_id)));
        }
        if let Some(columns) = columns {
            query.push(format!("columns={}", urlencoding::encode(columns)));
        }
        if let Some(format) = format {
            query.push(format!("format={}", urlencoding::encode(format)));
        }
        if let Some(scope) = scope {
            query.push(format!("scope={}", urlencoding::encode(scope)));
        }
        if let Some(include_archived) = include_archived {
            query.push(format!("include_archived={include_archived}"));
        }
        if let Some(limit) = limit {
            query.push(format!("limit={limit}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.get(&format!(
            "/api/v1/forms/{}/records/export{}",
            urlencoding::encode(form_id),
            query
        ))
        .await
    }

    pub async fn preview_import_form_records(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/forms/{}/records/import-preview", urlencoding::encode(form_id)),
            &body,
        )
        .await
    }

    pub async fn import_form_records(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/forms/{}/records/import", urlencoding::encode(form_id)),
            &body,
        )
        .await
    }

    pub async fn get_form_record(&self, record_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/form-records/{}", urlencoding::encode(record_id)))
            .await
    }

    pub async fn create_form_record(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/forms/{}/records", urlencoding::encode(form_id)),
            &body,
        )
        .await
    }

    pub async fn update_form_record(&self, record_id: &str, body: Value) -> Result<Value, String> {
        self.patch(
            &format!("/api/v1/form-records/{}", urlencoding::encode(record_id)),
            &body,
        )
        .await
    }

    pub async fn create_child_form_record(&self, record_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/form-records/{}/children", urlencoding::encode(record_id)),
            &body,
        )
        .await
    }

    pub async fn update_child_form_record(
        &self,
        record_id: &str,
        child_record_id: &str,
        body: Value,
    ) -> Result<Value, String> {
        self.patch(
            &format!(
                "/api/v1/form-records/{}/children/{}",
                urlencoding::encode(record_id),
                urlencoding::encode(child_record_id)
            ),
            &body,
        )
        .await
    }

    pub async fn archive_child_form_record(
        &self,
        record_id: &str,
        child_record_id: &str,
        relation_key: Option<&str>,
    ) -> Result<(), String> {
        let query = relation_key
            .map(|relation_key| format!("?relation_key={}", urlencoding::encode(relation_key)))
            .unwrap_or_default();
        self.delete(&format!(
            "/api/v1/form-records/{}/children/{}{}",
            urlencoding::encode(record_id),
            urlencoding::encode(child_record_id),
            query
        ))
        .await
    }

    pub async fn restore_child_form_record(
        &self,
        record_id: &str,
        child_record_id: &str,
        relation_key: Option<&str>,
    ) -> Result<Value, String> {
        let query = relation_key
            .map(|relation_key| format!("?relation_key={}", urlencoding::encode(relation_key)))
            .unwrap_or_default();
        self.post(
            &format!(
                "/api/v1/form-records/{}/children/{}/restore{}",
                urlencoding::encode(record_id),
                urlencoding::encode(child_record_id),
                query
            ),
            &json!({}),
        )
        .await
    }

    pub async fn link_form_record(&self, record_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/form-records/{}/links", urlencoding::encode(record_id)),
            &body,
        )
        .await
    }

    pub async fn aggregate_form_records(
        &self,
        form_id: &str,
        field_key: &str,
        aggregate: &str,
    ) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/forms/{}/aggregate?field_key={}&aggregate={}",
            urlencoding::encode(form_id),
            urlencoding::encode(field_key),
            urlencoding::encode(aggregate)
        ))
        .await
    }

    pub async fn list_form_relation_targets(
        &self,
        form_id: &str,
        field_key: Option<&str>,
        form_key: Option<&str>,
        q: Option<&str>,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<Value, String> {
        let mut query = Vec::new();
        if let Some(field_key) = field_key {
            query.push(format!("field_key={}", urlencoding::encode(field_key)));
        }
        if let Some(form_key) = form_key {
            query.push(format!("form_key={}", urlencoding::encode(form_key)));
        }
        if let Some(q) = q {
            query.push(format!("q={}", urlencoding::encode(q)));
        }
        if let Some(page) = page {
            query.push(format!("page={page}"));
        }
        if let Some(per_page) = per_page {
            query.push(format!("per_page={per_page}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.get(&format!(
            "/api/v1/forms/{}/relation-targets{}",
            urlencoding::encode(form_id),
            query
        ))
        .await
    }

    pub async fn list_form_record_children(
        &self,
        record_id: &str,
        relation_key: Option<&str>,
        child_form_key: Option<&str>,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<Value, String> {
        let mut query = Vec::new();
        if let Some(relation_key) = relation_key {
            query.push(format!("relation_key={}", urlencoding::encode(relation_key)));
        }
        if let Some(child_form_key) = child_form_key {
            query.push(format!("child_form_key={}", urlencoding::encode(child_form_key)));
        }
        if let Some(page) = page {
            query.push(format!("page={page}"));
        }
        if let Some(per_page) = per_page {
            query.push(format!("per_page={per_page}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.get(&format!(
            "/api/v1/form-records/{}/children{}",
            urlencoding::encode(record_id),
            query
        ))
        .await
    }

    pub async fn list_form_events(&self, form_id: &str, event_type: Option<&str>) -> Result<Value, String> {
        let path = event_type.map_or_else(
            || format!("/api/v1/forms/{}/events", urlencoding::encode(form_id)),
            |event_type| {
                format!(
                    "/api/v1/forms/{}/events?event_type={}",
                    urlencoding::encode(form_id),
                    urlencoding::encode(event_type)
                )
            },
        );
        self.get(&path).await
    }

    pub async fn list_form_record_events(&self, record_id: &str, event_type: Option<&str>) -> Result<Value, String> {
        let path = event_type.map_or_else(
            || format!("/api/v1/form-records/{}/events", urlencoding::encode(record_id)),
            |event_type| {
                format!(
                    "/api/v1/form-records/{}/events?event_type={}",
                    urlencoding::encode(record_id),
                    urlencoding::encode(event_type)
                )
            },
        );
        self.get(&path).await
    }

    // ---- Plugins ----

    pub async fn list_plugins(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{project_id}/plugins")).await
    }

    pub async fn get_plugin(&self, plugin_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/plugins/{}", urlencoding::encode(plugin_id)))
            .await
    }

    pub async fn install_plugin(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/projects/{project_id}/plugins"), &body)
            .await
    }

    pub async fn invoke_plugin(&self, plugin_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/plugins/{}/invoke", urlencoding::encode(plugin_id)),
            &body,
        )
        .await
    }

    pub async fn list_plugin_invocations(&self, plugin_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/plugins/{}/invocations",
            urlencoding::encode(plugin_id)
        ))
        .await
    }

    // ---- Project Context ----

    pub async fn get_project_context(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{project_id}/context")).await
    }

    pub async fn get_project_governance_context(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{project_id}/governance-context"))
            .await
    }

    pub async fn get_project_agent_policy(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{project_id}/agent-policy")).await
    }

    pub async fn get_project_release_readiness(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{project_id}/release-readiness"))
            .await
    }

    // ---- Work Items / Issues ----

    pub async fn list_work_items(&self, project_id: &str, page: u64, per_page: u64) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/projects/{project_id}/issues?page={page}&per_page={per_page}"
        ))
        .await
    }

    pub async fn get_work_item(&self, work_item_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/issues/{work_item_id}")).await
    }

    pub async fn create_work_item(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/projects/{project_id}/issues"), &body).await
    }

    pub async fn update_work_item(&self, work_item_id: &str, body: Value) -> Result<Value, String> {
        self.put(&format!("/api/v1/issues/{work_item_id}"), &body).await
    }

    pub async fn delete_work_item(&self, work_item_id: &str) -> Result<(), String> {
        self.delete(&format!("/api/v1/issues/{work_item_id}")).await
    }

    pub async fn search_work_items(&self, query: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/search?q={}&workspace_id={}",
            urlencoding::encode(query),
            self.workspace_id
        ))
        .await
    }

    pub async fn add_label_to_issue(&self, issue_id: &str, label_id: &str) -> Result<(), String> {
        let url = format!("{}/api/v1/issues/{issue_id}/labels/{label_id}", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "HTTP {status} from /api/v1/issues/{issue_id}/labels/{label_id}: {body}"
            ));
        }
        Ok(())
    }

    pub async fn remove_label_from_issue(&self, issue_id: &str, label_id: &str) -> Result<(), String> {
        self.delete(&format!("/api/v1/issues/{issue_id}/labels/{label_id}"))
            .await
    }

    pub async fn get_issue_labels(&self, issue_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/issues/{issue_id}/labels")).await
    }

    // ---- Comments ----

    pub async fn list_comments(&self, issue_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/issues/{issue_id}/comments")).await
    }

    pub async fn create_comment(&self, issue_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/issues/{issue_id}/comments"), &body).await
    }

    pub async fn delete_comment(&self, comment_id: &str) -> Result<(), String> {
        self.delete(&format!("/api/v1/comments/{comment_id}")).await
    }

    // ---- Uploads ----

    pub async fn upload_file(&self, filename: &str, content: Vec<u8>) -> Result<UploadedFile, String> {
        let url = format!("{}/api/v1/upload", self.base_url);
        let part = multipart::Part::bytes(content)
            .file_name(filename.to_string())
            .mime_str(detect_mime_type_from_filename(filename))
            .map_err(|e| format!("Failed to build multipart part: {e}"))?;
        let form = multipart::Form::new().part("file", part);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {status} from /api/v1/upload: {body}"));
        }

        let payload: Value = resp
            .json()
            .await
            .map_err(|e| format!("Failed to deserialize response: {e}"))?;

        let code = payload.get("code").and_then(Value::as_i64).unwrap_or(-1);
        if code != 0 {
            let message = payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("upload failed");
            return Err(format!("API error {code} from /api/v1/upload: {message}"));
        }

        let data = payload
            .get("data")
            .ok_or_else(|| "Missing data in /api/v1/upload response".to_string())?;

        let url = data
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing data.url in /api/v1/upload response".to_string())?
            .to_string();
        let filename = data
            .get("filename")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing data.filename in /api/v1/upload response".to_string())?
            .to_string();
        let storage_backend = data.get("storage_backend").and_then(Value::as_str).map(str::to_string);
        let object_key = data.get("object_key").and_then(Value::as_str).map(str::to_string);
        let thumbnail_url = data.get("thumbnail_url").and_then(Value::as_str).map(str::to_string);
        let thumbnail_object_key = data
            .get("thumbnail_object_key")
            .and_then(Value::as_str)
            .map(str::to_string);

        Ok(UploadedFile {
            url,
            filename,
            storage_backend,
            object_key,
            thumbnail_url,
            thumbnail_object_key,
        })
    }

    // ---- Proposals ----

    pub async fn list_proposals(&self, project_id: &str, status: Option<&str>) -> Result<Value, String> {
        use std::fmt::Write as _;
        let mut url = format!("/api/v1/proposals?project_id={project_id}");
        if let Some(s) = status {
            let _ = write!(url, "&status={s}");
        }
        self.get(&url).await
    }

    pub async fn get_proposal(&self, proposal_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/proposals/{proposal_id}")).await
    }

    pub async fn create_proposal(&self, body: Value) -> Result<Value, String> {
        self.post("/api/v1/proposals", &body).await
    }

    pub async fn create_check_result(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/projects/{project_id}/check-results"), &body)
            .await
    }

    pub async fn create_proposal_from_result(&self, check_result_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v1/check-results/{}/proposal",
                urlencoding::encode(check_result_id)
            ),
            &body,
        )
        .await
    }

    // ---- Members ----

    pub async fn list_members(&self) -> Result<Value, String> {
        self.get(&format!("/api/v1/workspaces/{}/members", self.workspace_id))
            .await
    }

    // ---- Sprints ----

    pub async fn create_sprint(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/projects/{project_id}/sprints"), &body)
            .await
    }

    pub async fn list_sprints(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{project_id}/sprints")).await
    }

    pub async fn update_sprint(&self, sprint_id: &str, body: Value) -> Result<Value, String> {
        self.put(&format!("/api/v1/sprints/{sprint_id}"), &body).await
    }

    pub async fn delete_sprint(&self, sprint_id: &str) -> Result<(), String> {
        self.delete(&format!("/api/v1/sprints/{sprint_id}")).await
    }

    // ---- Labels ----

    pub async fn create_label(&self, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/workspaces/{}/labels", self.workspace_id), &body)
            .await
    }

    pub async fn list_labels(&self) -> Result<Value, String> {
        self.get(&format!("/api/v1/workspaces/{}/labels", self.workspace_id))
            .await
    }

    pub async fn list_project_labels(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{project_id}/labels")).await
    }

    pub async fn get_work_item_by_identifier(&self, identifier: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/issues/by-identifier/{}",
            urlencoding::encode(identifier)
        ))
        .await
    }

    pub async fn add_labels_to_issue(&self, issue_id: &str, label_ids: &[String]) -> Result<(), String> {
        let url = format!("{}/api/v1/issues/{issue_id}/labels/batch", self.base_url);
        let body = serde_json::json!({ "label_ids": label_ids });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "HTTP {status} from /api/v1/issues/{issue_id}/labels/batch: {body}"
            ));
        }
        Ok(())
    }

    pub async fn update_label(&self, label_id: &str, body: Value) -> Result<Value, String> {
        self.put(&format!("/api/v1/labels/{label_id}"), &body).await
    }

    pub async fn delete_label(&self, label_id: &str) -> Result<(), String> {
        self.delete(&format!("/api/v1/labels/{label_id}")).await
    }

    // ---- Search All ----

    pub async fn search_all(&self, query: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/search?q={}&workspace_id={}",
            urlencoding::encode(query),
            self.workspace_id
        ))
        .await
    }
}

fn detect_mime_type_from_filename(filename: &str) -> &'static str {
    use std::path::Path;
    let path = Path::new(filename);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    // Special case: .tar.gz is a compound extension not handled by Path::extension
    if filename.to_ascii_lowercase().ends_with(".tar.gz") {
        return "application/gzip";
    }

    if ext.eq_ignore_ascii_case("png") {
        "image/png"
    } else if ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg") {
        "image/jpeg"
    } else if ext.eq_ignore_ascii_case("gif") {
        "image/gif"
    } else if ext.eq_ignore_ascii_case("webp") {
        "image/webp"
    } else if ext.eq_ignore_ascii_case("mp4") {
        "video/mp4"
    } else if ext.eq_ignore_ascii_case("webm") {
        "video/webm"
    } else if ext.eq_ignore_ascii_case("mov") {
        "video/quicktime"
    } else if ext.eq_ignore_ascii_case("avi") {
        "video/x-msvideo"
    } else if ext.eq_ignore_ascii_case("zip") {
        "application/zip"
    } else if ext.eq_ignore_ascii_case("gz") {
        "application/gzip"
    } else if ext.eq_ignore_ascii_case("log") || ext.eq_ignore_ascii_case("txt") {
        "text/plain"
    } else if ext.eq_ignore_ascii_case("pdf") {
        "application/pdf"
    } else if ext.eq_ignore_ascii_case("json") {
        "application/json"
    } else if ext.eq_ignore_ascii_case("csv") {
        "text/csv"
    } else if ext.eq_ignore_ascii_case("xml") {
        "application/xml"
    } else {
        "application/octet-stream"
    }
}

// Simple URL encoding helper (avoid extra dep)
mod urlencoding {
    use std::fmt::Write as _;

    pub fn encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(c),
                _ => {
                    for byte in c.to_string().as_bytes() {
                        let _ = write!(result, "%{byte:02X}");
                    }
                }
            }
        }
        result
    }
}
