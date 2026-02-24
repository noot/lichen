use eyre::{bail, Result, WrapErr as _};
use protocol::{CreateTaskRequest, SubmitRatingRequest, SubmitResultRequest, TaskStatus};
use uuid::Uuid;

/// HTTP client for the coordinator API.
pub struct CoordinatorClient {
    base_url: String,
    http: reqwest::Client,
}

impl std::fmt::Display for CoordinatorClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.base_url)
    }
}

impl CoordinatorClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// POST /tasks
    pub async fn create_task(&self, prompt: &str, num_raters: usize) -> Result<TaskStatus> {
        let resp = self
            .http
            .post(format!("{}/tasks", self.base_url))
            .json(&CreateTaskRequest {
                prompt: prompt.to_string(),
                num_raters,
            })
            .send()
            .await
            .wrap_err("failed to create task")?;
        check_status(&resp).wrap_err("create task returned error status")?;
        resp.json().await.wrap_err("failed to parse task")
    }

    /// GET /tasks
    pub async fn list_tasks(&self) -> Result<Vec<TaskStatus>> {
        let resp = self
            .http
            .get(format!("{}/tasks", self.base_url))
            .send()
            .await
            .wrap_err("failed to list tasks")?;
        check_status(&resp).wrap_err("list tasks returned error status")?;
        resp.json().await.wrap_err("failed to parse tasks")
    }

    /// GET /tasks/:id
    pub async fn get_task(&self, task_id: Uuid) -> Result<TaskStatus> {
        let resp = self
            .http
            .get(format!("{}/tasks/{task_id}", self.base_url))
            .send()
            .await
            .wrap_err("failed to get task")?;
        check_status(&resp).wrap_err("get task returned error status")?;
        resp.json().await.wrap_err("failed to parse task")
    }

    /// POST /tasks/:id/result
    pub async fn submit_result(
        &self,
        task_id: Uuid,
        agent_id: &str,
        output: &str,
    ) -> Result<TaskStatus> {
        let resp = self
            .http
            .post(format!("{}/tasks/{task_id}/result", self.base_url))
            .json(&SubmitResultRequest {
                task_id,
                agent_id: agent_id.to_string(),
                output: output.to_string(),
            })
            .send()
            .await
            .wrap_err("failed to submit result")?;
        check_status(&resp).wrap_err("submit result returned error status")?;
        resp.json()
            .await
            .wrap_err("failed to parse result response")
    }

    /// POST /tasks/:id/rating
    pub async fn submit_rating(
        &self,
        task_id: Uuid,
        agent_id: &str,
        signal: bool,
        prediction: f64,
    ) -> Result<TaskStatus> {
        let resp = self
            .http
            .post(format!("{}/tasks/{task_id}/rating", self.base_url))
            .json(&SubmitRatingRequest {
                task_id,
                agent_id: agent_id.to_string(),
                signal,
                prediction,
            })
            .send()
            .await
            .wrap_err("failed to submit rating")?;
        check_status(&resp).wrap_err("submit rating returned error status")?;
        resp.json()
            .await
            .wrap_err("failed to parse rating response")
    }

    /// POST /tasks/:id/rating (raw, returns status code for error testing)
    pub async fn submit_rating_raw(
        &self,
        task_id: Uuid,
        agent_id: &str,
        signal: bool,
        prediction: f64,
    ) -> Result<(reqwest::StatusCode, String)> {
        let resp = self
            .http
            .post(format!("{}/tasks/{task_id}/rating", self.base_url))
            .json(&SubmitRatingRequest {
                task_id,
                agent_id: agent_id.to_string(),
                signal,
                prediction,
            })
            .send()
            .await
            .wrap_err("failed to submit rating")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Ok((status, body))
    }

    /// POST /tasks/:id/result (raw, returns status code for error testing)
    pub async fn submit_result_raw(
        &self,
        task_id: Uuid,
        agent_id: &str,
        output: &str,
    ) -> Result<(reqwest::StatusCode, String)> {
        let resp = self
            .http
            .post(format!("{}/tasks/{task_id}/result", self.base_url))
            .json(&SubmitResultRequest {
                task_id,
                agent_id: agent_id.to_string(),
                output: output.to_string(),
            })
            .send()
            .await
            .wrap_err("failed to submit result")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Ok((status, body))
    }
}

fn check_status(resp: &reqwest::Response) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    bail!("request failed with status {status}");
}
