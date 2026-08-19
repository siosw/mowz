use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use eyre::{Context, ContextCompat, Result, bail};
use reqwest::{Client, header::CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{RESULT_LIMIT, TimeRange};

const API_URL: &str = "https://backboard.railway.com/graphql/v2";

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RailwayScope {
    #[default]
    Service,
    Environment,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RailwayAuth {
    ProjectToken,
    Bearer,
}

impl RailwayScope {
    pub(crate) fn filter(self, service_id: Option<&str>, filter: &str) -> Result<String> {
        match (self, service_id) {
            (Self::Service, Some(service_id)) if filter.is_empty() => {
                Ok(format!("@service:{service_id}"))
            }
            (Self::Service, Some(service_id)) => {
                Ok(format!("@service:{service_id} AND ({filter})"))
            }
            (Self::Service, None) => {
                bail!("Railway service scope requires service_id")
            }
            (Self::Environment, None) => Ok(filter.to_owned()),
            (Self::Environment, Some(_)) => {
                bail!("Railway environment scope must not configure service_id")
            }
        }
    }
}

pub(crate) async fn query_logs(
    client: &Client,
    token: &str,
    auth: RailwayAuth,
    environment_id: &str,
    filter: &str,
    time_range: &TimeRange,
) -> Result<Value> {
    query_logs_at(
        client,
        API_URL,
        token,
        auth,
        environment_id,
        filter,
        time_range,
    )
    .await
}

async fn query_logs_at(
    client: &Client,
    api_url: &str,
    token: &str,
    auth: RailwayAuth,
    environment_id: &str,
    filter: &str,
    time_range: &TimeRange,
) -> Result<Value> {
    let query = r#"query EnvironmentLogs($environmentId: String!, $filter: String, $beforeDate: String!, $anchorDate: String!, $afterDate: String!, $beforeLimit: Int!, $afterLimit: Int!) {
  environmentLogs(environmentId: $environmentId, filter: $filter, beforeDate: $beforeDate, anchorDate: $anchorDate, afterDate: $afterDate, beforeLimit: $beforeLimit, afterLimit: $afterLimit) {
    timestamp
    message
    severity
    tags {
      serviceId
      deploymentId
    }
  }
}"#;
    let (start_date, end_date) = resolve_time_range(time_range, Utc::now())?;
    let start_date = start_date.to_rfc3339_opts(SecondsFormat::Secs, true);
    let end_date = end_date.to_rfc3339_opts(SecondsFormat::Secs, true);

    query_api(
        client,
        api_url,
        token,
        auth,
        query,
        json!({
            "environmentId": environment_id,
            "filter": filter,
            "beforeDate": start_date,
            "anchorDate": end_date,
            "afterDate": end_date,
            "beforeLimit": RESULT_LIMIT + 1,
            "afterLimit": 0,
        }),
    )
    .await
}

fn resolve_time_range(
    time_range: &TimeRange,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let from = parse_time(time_range.from(), now)
        .wrap_err_with(|| format!("invalid --from value {:?} for Railway", time_range.from()))?;
    let to = parse_time(time_range.to(), now)
        .wrap_err_with(|| format!("invalid --to value {:?} for Railway", time_range.to()))?;
    if from > to {
        bail!("--from must not be later than --to");
    }
    Ok((from, to))
}

fn parse_time(value: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    if value == "now" {
        return Ok(now);
    }

    if let Some((sign, offset)) = value
        .strip_prefix("now-")
        .map(|offset| (-1, offset))
        .or_else(|| value.strip_prefix("now+").map(|offset| (1, offset)))
    {
        let (amount, unit) = offset.split_at(offset.len().saturating_sub(1));
        let amount = amount
            .parse::<i64>()
            .context("relative time must contain a positive whole number")?;
        if amount <= 0 {
            bail!("relative time must contain a positive whole number");
        }
        let seconds_per_unit = match unit {
            "s" => 1,
            "m" => 60,
            "h" => 60 * 60,
            "d" => 24 * 60 * 60,
            "w" => 7 * 24 * 60 * 60,
            _ => bail!("relative time unit must be one of s, m, h, d, or w"),
        };
        let seconds = amount
            .checked_mul(seconds_per_unit)
            .context("relative time is too large")?;
        let delta = TimeDelta::try_seconds(seconds).context("relative time is too large")?;
        return now
            .checked_add_signed(delta * sign)
            .context("relative time is out of range");
    }

    value
        .parse::<DateTime<Utc>>()
        .context("expected now, now-<duration>, now+<duration>, or an RFC 3339 timestamp")
}

async fn query_api(
    client: &Client,
    api_url: &str,
    token: &str,
    auth: RailwayAuth,
    query: &str,
    variables: Value,
) -> Result<Value> {
    let request = client
        .post(api_url)
        .header(CONTENT_TYPE, "application/json");
    let request = match auth {
        RailwayAuth::ProjectToken => request.header("Project-Access-Token", token),
        RailwayAuth::Bearer => request.bearer_auth(token),
    };
    let response = request
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .await
        .context("failed to query Railway")?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read Railway response body")?;
    if !status.is_success() {
        bail!("Railway query failed with status {status}: {body}");
    }

    let response: Value =
        serde_json::from_str(&body).context("failed to parse Railway response as JSON")?;
    if response
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|errors| !errors.is_empty())
    {
        bail!(
            "Railway query returned GraphQL errors: {}",
            response["errors"]
        );
    }
    Ok(response)
}

pub(crate) fn extract_entries(response: &Value) -> Vec<Map<String, Value>> {
    response
        .pointer("/data/environmentLogs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|entry| {
            let mut selected = ["timestamp", "message", "severity"]
                .into_iter()
                .filter_map(|field| {
                    entry
                        .get(field)
                        .filter(|value| !value.is_null())
                        .cloned()
                        .map(|value| (field.to_owned(), value))
                })
                .collect::<Map<_, _>>();
            if let Some(tags) = entry.get("tags").and_then(Value::as_object) {
                selected.extend(
                    ["serviceId", "deploymentId"]
                        .into_iter()
                        .filter_map(|field| {
                            tags.get(field)
                                .filter(|value| !value.is_null())
                                .cloned()
                                .map(|value| (field.to_owned(), value))
                        }),
                );
            }
            selected
        })
        .filter(|entry: &Map<String, Value>| !entry.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bound_entries;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string_contains, header, method, path},
    };

    #[test]
    fn parses_railway_logs_into_entries() {
        let response: Value =
            serde_json::from_str(include_str!("../tests/fixtures/railway-response.json")).unwrap();

        assert_eq!(
            extract_entries(&response),
            vec![Map::from_iter([
                ("deploymentId".to_owned(), json!("deployment-id")),
                ("message".to_owned(), json!("request completed")),
                ("severity".to_owned(), json!("info")),
                ("serviceId".to_owned(), json!("service-id")),
                ("timestamp".to_owned(), json!("2026-08-18T12:00:00Z")),
            ])]
        );
    }

    #[test]
    fn builds_explicit_railway_scope_filters() {
        assert_eq!(
            RailwayScope::Service
                .filter(Some("service-id"), "@level:error OR timeout")
                .unwrap(),
            "@service:service-id AND (@level:error OR timeout)"
        );
        assert_eq!(
            RailwayScope::Environment
                .filter(None, "@level:error OR timeout")
                .unwrap(),
            "@level:error OR timeout"
        );
        assert_eq!(
            RailwayScope::Service
                .filter(None, "error")
                .unwrap_err()
                .to_string(),
            "Railway service scope requires service_id"
        );
        assert_eq!(
            RailwayScope::Environment
                .filter(Some("service-id"), "error")
                .unwrap_err()
                .to_string(),
            "Railway environment scope must not configure service_id"
        );
    }

    #[test]
    fn resolves_relative_and_absolute_railway_times() {
        let now = "2026-08-19T12:00:00Z".parse::<DateTime<Utc>>().unwrap();

        assert_eq!(
            resolve_time_range(&TimeRange::new("now-6h", "now-30m"), now).unwrap(),
            (
                "2026-08-19T06:00:00Z".parse().unwrap(),
                "2026-08-19T11:30:00Z".parse().unwrap(),
            )
        );
        assert_eq!(
            resolve_time_range(
                &TimeRange::new("2026-08-18T12:00:00Z", "2026-08-19T12:00:00Z"),
                now,
            )
            .unwrap(),
            (
                "2026-08-18T12:00:00Z".parse().unwrap(),
                "2026-08-19T12:00:00Z".parse().unwrap(),
            )
        );
        assert_eq!(
            resolve_time_range(&TimeRange::new("now", "now-1h"), now)
                .unwrap_err()
                .to_string(),
            "--from must not be later than --to"
        );
    }

    #[tokio::test]
    async fn queries_bounded_railway_environment_logs() {
        let server = MockServer::start().await;
        let logs = (0..101)
            .map(|number| {
                json!({
                    "timestamp": format!("2026-08-18T12:00:{:02}Z", number % 60),
                    "message": format!("line {number}"),
                    "severity": "info",
                    "tags": {
                        "serviceId": "service-id",
                        "deploymentId": format!("deployment-{number}"),
                    },
                })
            })
            .collect::<Vec<_>>();
        Mock::given(method("POST"))
            .and(path("/graphql/v2"))
            .and(header("project-access-token", "secret-token"))
            .and(body_string_contains("query EnvironmentLogs("))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "environmentLogs": logs }
            })))
            .mount(&server)
            .await;

        let api_url = format!("{}/graphql/v2", server.uri());
        let client = Client::new();
        let response = query_logs_at(
            &client,
            &api_url,
            "secret-token",
            RailwayAuth::ProjectToken,
            "environment-id",
            "@service:service-id AND (@level:error OR timeout)",
            &TimeRange::new("now-6h", "now-30m"),
        )
        .await
        .unwrap();
        let entries = bound_entries(extract_entries(&response), true);
        assert_eq!(entries.len(), 100);
        assert_eq!(entries[0]["message"], "line 1");
        assert_eq!(entries[0]["serviceId"], "service-id");
        assert_eq!(entries[0]["deploymentId"], "deployment-1");
        assert_eq!(entries[99]["message"], "line 100");

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let logs_request: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(logs_request["variables"]["environmentId"], "environment-id");
        assert_eq!(logs_request["variables"]["beforeLimit"], 101);
        assert_eq!(logs_request["variables"]["afterLimit"], 0);
        assert_eq!(
            logs_request["variables"]["filter"],
            "@service:service-id AND (@level:error OR timeout)"
        );
        let start_date = logs_request["variables"]["beforeDate"]
            .as_str()
            .unwrap()
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();
        let age = Utc::now() - start_date;
        assert!(age >= TimeDelta::minutes(359) && age <= TimeDelta::minutes(361));
        assert_eq!(
            logs_request["variables"]["anchorDate"],
            logs_request["variables"]["afterDate"]
        );
        let end_date = logs_request["variables"]["anchorDate"]
            .as_str()
            .unwrap()
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();
        let age = Utc::now() - end_date;
        assert!(age >= TimeDelta::minutes(29) && age <= TimeDelta::minutes(31));
    }

    #[tokio::test]
    async fn reports_railway_graphql_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql/v2"))
            .and(header("authorization", "Bearer secret-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "errors": [{ "message": "not authorized" }]
            })))
            .mount(&server)
            .await;

        let error = query_api(
            &Client::new(),
            &format!("{}/graphql/v2", server.uri()),
            "secret-token",
            RailwayAuth::Bearer,
            "query Test { me { id } }",
            json!({}),
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Railway query returned GraphQL errors")
        );
    }
}
