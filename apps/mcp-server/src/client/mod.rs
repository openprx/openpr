use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone)]
pub struct OpenPrClient {
    client: Client,
    base_url: String,
    bot_token: String,
    pub workspace_id: String,
}

impl OpenPrClient {
    pub fn new(base_url: String, bot_token: String, workspace_id: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client"),
            base_url,
            bot_token,
            workspace_id,
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {} from {}: {}", status, path, body));
        }

        resp.json::<T>()
            .await
            .map_err(|e| format!("Failed to deserialize response: {}", e))
    }

    pub async fn post<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {} from {}: {}", status, path, body));
        }

        resp.json::<T>()
            .await
            .map_err(|e| format!("Failed to deserialize response: {}", e))
    }

    pub async fn patch<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .patch(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {} from {}: {}", status, path, body));
        }

        resp.json::<T>()
            .await
            .map_err(|e| format!("Failed to deserialize response: {}", e))
    }

    pub async fn delete(&self, path: &str) -> Result<(), String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {} from {}: {}", status, path, body));
        }

        Ok(())
    }

    // ---- Projects ----

    pub async fn list_projects(&self) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/workspaces/{}/projects",
            self.workspace_id
        ))
        .await
    }

    pub async fn get_project(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{}", project_id)).await
    }

    pub async fn create_project(&self, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/workspaces/{}/projects", self.workspace_id),
            &body,
        )
        .await
    }

    pub async fn update_project(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.patch(&format!("/api/v1/projects/{}", project_id), &body)
            .await
    }

    pub async fn delete_project(&self, project_id: &str) -> Result<(), String> {
        self.delete(&format!("/api/v1/projects/{}", project_id))
            .await
    }

    // ---- Work Items / Issues ----

    pub async fn list_work_items(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{}/issues", project_id))
            .await
    }

    pub async fn get_work_item(&self, work_item_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/issues/{}", work_item_id)).await
    }

    pub async fn create_work_item(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/projects/{}/issues", project_id), &body)
            .await
    }

    pub async fn update_work_item(&self, work_item_id: &str, body: Value) -> Result<Value, String> {
        self.patch(&format!("/api/v1/issues/{}", work_item_id), &body)
            .await
    }

    pub async fn delete_work_item(&self, work_item_id: &str) -> Result<(), String> {
        self.delete(&format!("/api/v1/issues/{}", work_item_id))
            .await
    }

    pub async fn search_work_items(&self, query: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/search?q={}&workspace_id={}",
            urlencoding::encode(query),
            self.workspace_id
        ))
        .await
    }

    // ---- Comments ----

    pub async fn list_comments(&self, issue_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/issues/{}/comments", issue_id))
            .await
    }

    pub async fn create_comment(&self, issue_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/issues/{}/comments", issue_id), &body)
            .await
    }

    pub async fn delete_comment(&self, comment_id: &str) -> Result<(), String> {
        self.delete(&format!("/api/v1/comments/{}", comment_id))
            .await
    }

    // ---- Proposals ----

    pub async fn list_proposals(
        &self,
        project_id: &str,
        status: Option<&str>,
    ) -> Result<Value, String> {
        let mut url = format!("/api/v1/proposals?project_id={}", project_id);
        if let Some(s) = status {
            url.push_str(&format!("&status={}", s));
        }
        self.get(&url).await
    }

    pub async fn get_proposal(&self, proposal_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/proposals/{}", proposal_id))
            .await
    }

    pub async fn create_proposal(&self, body: Value) -> Result<Value, String> {
        self.post("/api/v1/proposals", &body).await
    }

    // ---- Members ----

    pub async fn list_members(&self) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/workspaces/{}/members",
            self.workspace_id
        ))
        .await
    }

    // ---- Sprints ----

    pub async fn create_sprint(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/projects/{}/sprints", project_id), &body)
            .await
    }

    pub async fn update_sprint(&self, sprint_id: &str, body: Value) -> Result<Value, String> {
        self.patch(&format!("/api/v1/sprints/{}", sprint_id), &body)
            .await
    }

    // ---- Labels ----

    pub async fn create_label(&self, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/workspaces/{}/labels", self.workspace_id),
            &body,
        )
        .await
    }

    pub async fn list_labels(&self) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/workspaces/{}/labels",
            self.workspace_id
        ))
        .await
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

// Simple URL encoding helper (avoid extra dep)
mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(c),
                _ => {
                    for byte in c.to_string().as_bytes() {
                        result.push_str(&format!("%{:02X}", byte));
                    }
                }
            }
        }
        result
    }
}
