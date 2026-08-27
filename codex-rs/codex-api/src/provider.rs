use codex_client::Request;
use codex_client::RequestCompression;
use codex_client::RetryOn;
use codex_client::RetryPolicy;
use http::Method;
use http::header::HeaderMap;
use std::collections::HashMap;
use std::time::Duration;
use url::Url;

/// High-level retry configuration for a provider.
///
/// This is converted into a `RetryPolicy` used by `codex-client` to drive
/// transport-level retries for both unary and streaming calls.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_attempts: u64,
    pub base_delay: Duration,
    pub retry_429: bool,
    pub retry_5xx: bool,
    pub retry_transport: bool,
}

impl RetryConfig {
    pub fn to_policy(&self) -> RetryPolicy {
        RetryPolicy {
            max_attempts: self.max_attempts,
            base_delay: self.base_delay,
            retry_on: RetryOn {
                retry_429: self.retry_429,
                retry_5xx: self.retry_5xx,
                retry_transport: self.retry_transport,
            },
        }
    }
}

/// Resolved deployment information used to construct provider request targets.
///
/// This contains the provider deployment base URL plus query parameters that apply to every
/// operation against that deployment. Protocol operation paths remain owned by endpoint clients.
#[derive(Debug, Clone)]
pub struct ApiDeployment {
    pub base_url: String,
    pub query_params: Option<HashMap<String, String>>,
}

impl ApiDeployment {
    pub fn url_for_path(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        let mut url = if path.is_empty() {
            base.to_string()
        } else {
            format!("{base}/{path}")
        };

        if let Some(params) = &self.query_params
            && !params.is_empty()
        {
            let qs = params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            url.push('?');
            url.push_str(&qs);
        }

        url
    }

    pub fn websocket_url_for_path(&self, path: &str) -> Result<Url, url::ParseError> {
        let mut url = Url::parse(&self.url_for_path(path))?;

        let scheme = match url.scheme() {
            "http" => "ws",
            "https" => "wss",
            "ws" | "wss" => return Ok(url),
            _ => return Ok(url),
        };
        let _ = url.set_scheme(scheme);
        Ok(url)
    }
}

/// Request/stream execution policy for a resolved provider setup.
#[derive(Debug, Clone)]
pub struct RequestExecutionPolicy {
    pub retry: RetryConfig,
    pub stream_idle_timeout: Duration,
}

/// Resolved low-level provider setup consumed by API clients.
///
/// Endpoint/deployment location and request execution policy are separate subobjects. Default
/// headers remain transitional request decoration until their ownership is extracted independently.
#[derive(Debug, Clone)]
pub struct Provider {
    pub name: String,
    pub deployment: ApiDeployment,
    pub headers: HeaderMap,
    pub request_policy: RequestExecutionPolicy,
}

impl Provider {
    pub fn build_request(&self, method: Method, path: &str) -> Request {
        Request {
            method,
            url: self.deployment.url_for_path(path),
            headers: self.headers.clone(),
            body: None,
            compression: RequestCompression::None,
            timeout: None,
        }
    }
}

pub fn is_azure_responses_provider(name: &str, base_url: Option<&str>) -> bool {
    if name.eq_ignore_ascii_case("azure") {
        true
    } else if let Some(base_url) = base_url {
        matches_azure_responses_base_url(base_url)
    } else {
        false
    }
}

fn matches_azure_responses_base_url(base_url: &str) -> bool {
    let base_url = base_url.to_ascii_lowercase();
    const AZURE_MARKERS: [&str; 6] = [
        "openai.azure.",
        "cognitiveservices.azure.",
        "aoai.azure.",
        "azure-api.",
        "azurefd.",
        "windows.net/openai",
    ];
    AZURE_MARKERS.iter().any(|marker| base_url.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_builds_operation_url_with_query_defaults() {
        let deployment = ApiDeployment {
            base_url: "https://example.com/v1/".to_string(),
            query_params: Some(HashMap::from([(
                "api-version".to_string(),
                "2025-04-01-preview".to_string(),
            )])),
        };

        assert_eq!(
            deployment.url_for_path("/responses"),
            "https://example.com/v1/responses?api-version=2025-04-01-preview"
        );
    }

    #[test]
    fn deployment_converts_http_scheme_for_websocket_operation() {
        let deployment = ApiDeployment {
            base_url: "https://example.com/v1".to_string(),
            query_params: None,
        };

        assert_eq!(
            deployment
                .websocket_url_for_path("responses")
                .expect("websocket URL should build")
                .as_str(),
            "wss://example.com/v1/responses"
        );
    }

    #[test]
    fn detects_azure_responses_base_urls() {
        let positive_cases = [
            "https://foo.openai.azure.com/openai",
            "https://foo.openai.azure.us/openai/deployments/bar",
            "https://foo.cognitiveservices.azure.cn/openai",
            "https://foo.aoai.azure.com/openai",
            "https://foo.openai.azure-api.net/openai",
            "https://foo.z01.azurefd.net/",
        ];

        for base_url in positive_cases {
            assert!(
                is_azure_responses_provider("test", Some(base_url)),
                "expected {base_url} to be detected as Azure"
            );
        }

        assert!(is_azure_responses_provider(
            "Azure",
            Some("https://example.com")
        ));

        let negative_cases = [
            "https://api.openai.com/v1",
            "https://example.com/openai",
            "https://myproxy.azurewebsites.net/openai",
        ];

        for base_url in negative_cases {
            assert!(
                !is_azure_responses_provider("test", Some(base_url)),
                "expected {base_url} not to be detected as Azure"
            );
        }
    }
}
