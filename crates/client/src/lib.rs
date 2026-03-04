use eyre::{bail, Result, WrapErr as _};
use protocol::{
    AcceptTaskRequest, AgentRole, CancelTaskRequest, CreateTaskRequest, DeclineTaskRequest,
    FinalizeTaskRequest, SubmitRatingRequest, SubmitResultRequest, SubscribeRequest,
    SubscribeResponse, TaskAcceptResponse, TaskStatus,
};
use serde_json::Value as JsonValue;
use uuid::Uuid;

/// HTTP client for the coordinator API.
pub struct CoordinatorClient {
    pub base_url: String,
    pub http: reqwest::Client,
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
                output: String::new(),
                num_raters,
                max_raters: None,
                min_raters: None,
                timeout_seconds: None,
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

    /// POST /subscribe
    pub async fn subscribe(
        &self,
        agent_id: &str,
        callback_url: &str,
        roles: Vec<AgentRole>,
    ) -> Result<SubscribeResponse> {
        let resp = self
            .http
            .post(format!("{}/subscribe", self.base_url))
            .json(&SubscribeRequest {
                agent_id: agent_id.to_string(),
                callback_url: callback_url.to_string(),
                roles,
            })
            .send()
            .await
            .wrap_err("failed to subscribe")?;
        check_status(&resp).wrap_err("subscribe returned error status")?;
        resp.json().await.wrap_err("failed to parse subscribe response")
    }

    /// DELETE /subscribe/:agent_id
    pub async fn unsubscribe(&self, agent_id: &str) -> Result<JsonValue> {
        let resp = self
            .http
            .delete(format!("{}/subscribe/{agent_id}", self.base_url))
            .send()
            .await
            .wrap_err("failed to unsubscribe")?;
        check_status(&resp).wrap_err("unsubscribe returned error status")?;
        resp.json().await.wrap_err("failed to parse unsubscribe response")
    }

    /// GET /subscriptions
    pub async fn list_subscriptions(&self) -> Result<JsonValue> {
        let resp = self
            .http
            .get(format!("{}/subscriptions", self.base_url))
            .send()
            .await
            .wrap_err("failed to list subscriptions")?;
        check_status(&resp).wrap_err("list subscriptions returned error status")?;
        resp.json().await.wrap_err("failed to parse subscriptions")
    }

    /// POST /tasks/:id/accept
    pub async fn accept_task(
        &self,
        task_id: Uuid,
        agent_id: &str,
        role: AgentRole,
    ) -> Result<TaskAcceptResponse> {
        let resp = self
            .http
            .post(format!("{}/tasks/{task_id}/accept", self.base_url))
            .json(&AcceptTaskRequest {
                agent_id: agent_id.to_string(),
                role,
            })
            .send()
            .await
            .wrap_err("failed to accept task")?;
        check_status(&resp).wrap_err("accept task returned error status")?;
        resp.json().await.wrap_err("failed to parse accept response")
    }

    /// POST /tasks/:id/accept (raw)
    pub async fn accept_task_raw(
        &self,
        task_id: Uuid,
        agent_id: &str,
        role: AgentRole,
    ) -> Result<(reqwest::StatusCode, String)> {
        let resp = self
            .http
            .post(format!("{}/tasks/{task_id}/accept", self.base_url))
            .json(&AcceptTaskRequest {
                agent_id: agent_id.to_string(),
                role,
            })
            .send()
            .await
            .wrap_err("failed to accept task")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Ok((status, body))
    }

    /// POST /tasks/:id/decline
    pub async fn decline_task(
        &self,
        task_id: Uuid,
        agent_id: &str,
        reason: &str,
    ) -> Result<TaskAcceptResponse> {
        let resp = self
            .http
            .post(format!("{}/tasks/{task_id}/decline", self.base_url))
            .json(&DeclineTaskRequest {
                agent_id: agent_id.to_string(),
                reason: reason.to_string(),
            })
            .send()
            .await
            .wrap_err("failed to decline task")?;
        check_status(&resp).wrap_err("decline task returned error status")?;
        resp.json().await.wrap_err("failed to parse decline response")
    }

    /// POST /tasks/:id/finalize
    pub async fn finalize_task(
        &self,
        task_id: Uuid,
        agent_id: &str,
    ) -> Result<(reqwest::StatusCode, String)> {
        let resp = self
            .http
            .post(format!("{}/tasks/{task_id}/finalize", self.base_url))
            .json(&FinalizeTaskRequest {
                agent_id: agent_id.to_string(),
            })
            .send()
            .await
            .wrap_err("failed to finalize task")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Ok((status, body))
    }

    /// POST /tasks/:id/cancel
    pub async fn cancel_task(
        &self,
        task_id: Uuid,
        agent_id: &str,
    ) -> Result<(reqwest::StatusCode, String)> {
        let resp = self
            .http
            .post(format!("{}/tasks/{task_id}/cancel", self.base_url))
            .json(&CancelTaskRequest {
                agent_id: agent_id.to_string(),
            })
            .send()
            .await
            .wrap_err("failed to cancel task")?;
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
