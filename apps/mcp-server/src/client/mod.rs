use reqwest::{Client, multipart};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

#[derive(Clone)]
pub struct OpenPrClient {
    client: Client,
    base_url: String,
    bot_token: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone)]
pub struct UploadedFile {
    pub url: String,
    pub filename: String,
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

    pub async fn put<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .put(&url)
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
        self.put(&format!("/api/v1/issues/{}", work_item_id), &body)
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

    pub async fn add_label_to_issue(&self, issue_id: &str, label_id: &str) -> Result<(), String> {
        let url = format!(
            "{}/api/v1/issues/{}/labels/{}",
            self.base_url, issue_id, label_id
        );
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "HTTP {} from /api/v1/issues/{}/labels/{}: {}",
                status, issue_id, label_id, body
            ));
        }
        Ok(())
    }

    pub async fn remove_label_from_issue(
        &self,
        issue_id: &str,
        label_id: &str,
    ) -> Result<(), String> {
        self.delete(&format!("/api/v1/issues/{}/labels/{}", issue_id, label_id))
            .await
    }

    pub async fn get_issue_labels(&self, issue_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/issues/{}/labels", issue_id))
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

    // ---- Uploads ----

    pub async fn upload_file(
        &self,
        filename: &str,
        content: Vec<u8>,
    ) -> Result<UploadedFile, String> {
        let url = format!("{}/api/v1/upload", self.base_url);
        let part = multipart::Part::bytes(content)
            .file_name(filename.to_string())
            .mime_str(detect_mime_type_from_filename(filename))
            .map_err(|e| format!("Failed to build multipart part: {}", e))?;
        let form = multipart::Form::new().part("file", part);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {} from /api/v1/upload: {}", status, body));
        }

        let payload: Value = resp
            .json()
            .await
            .map_err(|e| format!("Failed to deserialize response: {}", e))?;

        let code = payload.get("code").and_then(Value::as_i64).unwrap_or(-1);
        if code != 0 {
            let message = payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("upload failed");
            return Err(format!("API error {} from /api/v1/upload: {}", code, message));
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

        Ok(UploadedFile { url, filename })
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
        self.get(&format!("/api/v1/workspaces/{}/members", self.workspace_id))
            .await
    }

    // ---- Sprints ----

    pub async fn create_sprint(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(&format!("/api/v1/projects/{}/sprints", project_id), &body)
            .await
    }

    pub async fn list_sprints(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{}/sprints", project_id))
            .await
    }

    pub async fn update_sprint(&self, sprint_id: &str, body: Value) -> Result<Value, String> {
        self.put(&format!("/api/v1/sprints/{}", sprint_id), &body)
            .await
    }

    pub async fn delete_sprint(&self, sprint_id: &str) -> Result<(), String> {
        self.delete(&format!("/api/v1/sprints/{}", sprint_id)).await
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
        self.get(&format!("/api/v1/workspaces/{}/labels", self.workspace_id))
            .await
    }

    pub async fn list_project_labels(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{}/labels", project_id))
            .await
    }

    pub async fn get_work_item_by_identifier(&self, identifier: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/issues/by-identifier/{}",
            urlencoding::encode(identifier)
        ))
        .await
    }

    pub async fn add_labels_to_issue(
        &self,
        issue_id: &str,
        label_ids: &[String],
    ) -> Result<(), String> {
        let url = format!("{}/api/v1/issues/{}/labels/batch", self.base_url, issue_id);
        let body = serde_json::json!({ "label_ids": label_ids });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "HTTP {} from /api/v1/issues/{}/labels/batch: {}",
                status, issue_id, body
            ));
        }
        Ok(())
    }

    pub async fn update_label(&self, label_id: &str, body: Value) -> Result<Value, String> {
        self.put(&format!("/api/v1/labels/{}", label_id), &body)
            .await
    }

    pub async fn delete_label(&self, label_id: &str) -> Result<(), String> {
        self.delete(&format!("/api/v1/labels/{}", label_id)).await
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
    let name = filename.to_ascii_lowercase();

    if name.ends_with(".tar.gz") {
        "application/gzip"
    } else if name.ends_with(".png") {
        "image/png"
    } else if name.ends_with(".jpg") || name.ends_with(".jpeg") {
        "image/jpeg"
    } else if name.ends_with(".gif") {
        "image/gif"
    } else if name.ends_with(".webp") {
        "image/webp"
    } else if name.ends_with(".mp4") {
        "video/mp4"
    } else if name.ends_with(".webm") {
        "video/webm"
    } else if name.ends_with(".mov") {
        "video/quicktime"
    } else if name.ends_with(".avi") {
        "video/x-msvideo"
    } else if name.ends_with(".zip") {
        "application/zip"
    } else if name.ends_with(".gz") {
        "application/gzip"
    } else if name.ends_with(".log") || name.ends_with(".txt") {
        "text/plain"
    } else if name.ends_with(".pdf") {
        "application/pdf"
    } else if name.ends_with(".json") {
        "application/json"
    } else if name.ends_with(".csv") {
        "text/csv"
    } else if name.ends_with(".xml") {
        "application/xml"
    } else {
        "application/octet-stream"
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
