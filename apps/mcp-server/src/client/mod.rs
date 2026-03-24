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
        })
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

        Ok(UploadedFile { url, filename })
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
