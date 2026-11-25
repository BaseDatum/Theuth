//! `aws-smithy-mocks` rules per operation (221 §13 B3). The localstack suite is `#[ignore]`.

use super::*;
use aws_credential_types::provider::{ProvideCredentials, future};
use aws_sdk_secretsmanager::error::ErrorMetadata;
use aws_sdk_secretsmanager::operation::create_secret::{CreateSecretError, CreateSecretOutput};
use aws_sdk_secretsmanager::operation::delete_secret::DeleteSecretOutput;
use aws_sdk_secretsmanager::operation::get_secret_value::{
    GetSecretValueError, GetSecretValueOutput,
};
use aws_sdk_secretsmanager::operation::list_secrets::ListSecretsOutput;
use aws_sdk_secretsmanager::operation::put_secret_value::PutSecretValueOutput;
use aws_sdk_secretsmanager::primitives::Blob;
use aws_sdk_secretsmanager::types::SecretListEntry;
use aws_sdk_secretsmanager::types::error::{ResourceExistsException, ResourceNotFoundException};
use aws_smithy_mocks::{Rule, RuleMode, mock, mock_client};

fn cfg() -> AwsConnConfig {
    AwsConnConfig::new("prod", "eu-west-1")
}

fn backend(cfg: AwsConnConfig, rules: &[&Rule]) -> AwsBackend {
    AwsBackend::with_client(
        cfg,
        mock_client!(aws_sdk_secretsmanager, RuleMode::MatchAny, rules),
    )
}

fn at(id: &str, key: Option<&str>, back: u8) -> ExtRef {
    ExtRef {
        conn: "prod".into(),
        id: id.into(),
        key: key.map(Into::into),
        back,
    }
}

fn meta(code: &str) -> ErrorMetadata {
    ErrorMetadata::builder()
        .code(code)
        .message("from AWS")
        .build()
}

fn stages(pairs: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(id, s)| (id.to_string(), s.iter().map(|x| x.to_string()).collect()))
        .collect()
}

fn describe_rule(current: &'static str) -> Rule {
    mock!(Client::describe_secret).then_output(move || {
        DescribeSecretOutput::builder()
            .name("app/db")
            .set_version_ids_to_stages(Some(stages(&[(current, &[CURRENT]), ("v0", &[PREVIOUS])])))
            .build()
    })
}

fn get_rule(stage: &'static str, id: &'static str, value: &'static str) -> Rule {
    mock!(Client::get_secret_value)
        .match_requests(move |r| r.version_stage() == Some(stage))
        .then_output(move || {
            GetSecretValueOutput::builder()
                .name("app/db")
                .version_id(id)
                .version_stages(stage)
                .secret_string(value)
                .build()
        })
}

#[tokio::test]
async fn get_reads_the_whole_string_a_json_key_and_awsprevious() {
    let current = get_rule(
        CURRENT,
        "v2",
        r#"{"user":"app","port":5432,"tls":true,"nested":{}}"#,
    );
    let previous = get_rule(PREVIOUS, "v1", "old");
    let b = backend(cfg(), &[&current, &previous]);

    let (whole, v) = b.get(&at("app/db", None, 0)).await.unwrap();
    assert!(whole.starts_with(r#"{"user""#));
    assert_eq!(
        (v.id.as_str(), v.stages.as_slice()),
        ("v2", &[CURRENT.to_owned()][..])
    );
    for (key, want) in [("user", "app"), ("port", "5432"), ("tls", "true")] {
        let (got, _) = b.get(&at("app/db", Some(key), 0)).await.unwrap();
        assert_eq!(got.as_str(), want, "#{key}");
    }
    assert_eq!(
        b.get(&at("app/db", Some("missing"), 0)).await.unwrap_err(),
        ExtError::NotFound
    );
    assert!(matches!(
        b.get(&at("app/db", Some("nested"), 0)).await.unwrap_err(),
        ExtError::Unsupported(_)
    ));

    let (old, v) = b.get(&at("app/db", None, 1)).await.unwrap();
    assert_eq!((old.as_str(), v.id.as_str()), ("old", "v1"));
    assert!(matches!(
        b.get(&at("app/db", Some("user"), 1)).await.unwrap_err(),
        ExtError::Unsupported(_)
    ));
    // Only `:-1`: a deeper count is refused before any request.
    let calls = current.num_calls() + previous.num_calls();
    assert!(matches!(
        b.get(&at("app/db", None, 2)).await.unwrap_err(),
        ExtError::Unsupported(_)
    ));
    assert_eq!(current.num_calls() + previous.num_calls(), calls);
    assert_eq!(b.status(), ConnStatus::Ready);
}

#[tokio::test]
async fn binary_and_missing_secrets_map_to_their_errors() {
    let binary = mock!(Client::get_secret_value)
        .match_requests(|r| r.secret_id() == Some("bin"))
        .then_output(|| {
            GetSecretValueOutput::builder()
                .secret_binary(Blob::new(b"\x00\x01".to_vec()))
                .build()
        });
    let missing = mock!(Client::get_secret_value)
        .match_requests(|r| r.secret_id() == Some("gone"))
        .then_error(|| {
            GetSecretValueError::ResourceNotFoundException(
                ResourceNotFoundException::builder()
                    .meta(meta("ResourceNotFoundException"))
                    .build(),
            )
        });
    let b = backend(cfg(), &[&binary, &missing]);
    assert_eq!(
        b.get(&at("bin", None, 0)).await.unwrap_err(),
        ExtError::Unsupported("binary secret".into())
    );
    assert_eq!(
        b.get(&at("gone", None, 0)).await.unwrap_err(),
        ExtError::NotFound
    );
}

#[tokio::test]
async fn create_writes_a_json_key_with_the_default_kms_and_maps_exists() {
    let created = mock!(Client::create_secret)
        .match_requests(|r| {
            r.name() == Some("app/ci/token")
                && r.secret_string() == Some(r#"{"token":"s3cr3t"}"#)
                && r.kms_key_id() == Some("alias/app")
                && r.client_request_token().is_some()
        })
        .then_output(|| {
            CreateSecretOutput::builder()
                .name("app/ci/token")
                .version_id("v1")
                .build()
        });
    let exists = mock!(Client::create_secret)
        .match_requests(|r| r.name() == Some("app/taken"))
        .then_error(|| {
            CreateSecretError::ResourceExistsException(
                ResourceExistsException::builder()
                    .meta(meta("ResourceExistsException"))
                    .build(),
            )
        });
    let mut c = cfg();
    c.kms_key_id = Some("alias/app".into());
    let b = backend(c, &[&created, &exists]);
    let loc = |name: &str| Location::Aws {
        name: name.into(),
        kms_key_id: None,
        description: None,
        json_key: Some("token".into()),
    };

    let (r, v) = b
        .create(&loc("app/ci/token"), Value::Plain("s3cr3t"))
        .await
        .unwrap();
    assert_eq!(r, at("app/ci/token", Some("token"), 0));
    assert_eq!(v.id, "v1");
    assert_eq!(
        b.create(&loc("app/taken"), Value::Plain("x"))
            .await
            .unwrap_err(),
        ExtError::Exists
    );
    // An invalid name never reaches AWS.
    assert!(matches!(
        b.create(&loc("bad name"), Value::Plain("x"))
            .await
            .unwrap_err(),
        ExtError::Unsupported(_)
    ));
    assert_eq!(created.num_calls() + exists.num_calls(), 2);
}

#[tokio::test]
async fn update_with_a_stale_version_is_a_conflict_and_writes_nothing() {
    let describe = describe_rule("v2");
    let b = backend(cfg(), &[&describe]);
    let stale = ExtVersion {
        id: "v1".into(),
        stages: vec![CURRENT.into()],
        changed: None,
    };
    let err = b
        .update(&at("app/db", None, 0), Value::Plain("new"), Some(&stale))
        .await
        .unwrap_err();
    let ExtError::Conflict { current } = err else {
        panic!("{err:?}")
    };
    assert_eq!(current.id, "v2");
}

#[tokio::test]
async fn update_of_a_json_key_keeps_the_other_keys() {
    let describe = describe_rule("v2");
    let read = get_rule(CURRENT, "v2", r#"{"password":"old","user":"app"}"#);
    let put = mock!(Client::put_secret_value)
        .match_requests(|r| {
            r.secret_id() == Some("app/db")
                && r.secret_string() == Some(r#"{"password":"new","user":"app"}"#)
                && r.client_request_token().is_some()
        })
        .then_output(|| {
            PutSecretValueOutput::builder()
                .version_id("v3")
                .version_stages(CURRENT)
                .build()
        });
    let b = backend(cfg(), &[&describe, &read, &put]);
    let seen = ExtVersion {
        id: "v2".into(),
        stages: vec![CURRENT.into()],
        changed: None,
    };
    let v = b
        .update(
            &at("app/db", Some("password"), 0),
            Value::Plain("new"),
            Some(&seen),
        )
        .await
        .unwrap();
    assert_eq!(v.id, "v3");
    assert_eq!(put.num_calls(), 1);
}

#[tokio::test]
async fn delete_is_recoverable_for_30_days_and_permanent_only_when_allowed() {
    let recoverable = mock!(Client::delete_secret)
        .match_requests(|r| {
            r.recovery_window_in_days() == Some(30) && r.force_delete_without_recovery().is_none()
        })
        .then_output(|| {
            DeleteSecretOutput::builder()
                .deletion_date(DateTime::from_secs(1_800_000_000))
                .build()
        });
    let permanent = mock!(Client::delete_secret)
        .match_requests(|r| {
            r.force_delete_without_recovery() == Some(true) && r.recovery_window_in_days().is_none()
        })
        .then_output(|| {
            DeleteSecretOutput::builder()
                .deletion_date(DateTime::from_secs(1_700_000_000))
                .build()
        });

    let b = backend(cfg(), &[&recoverable, &permanent]);
    let out = b
        .delete(
            &at("app/db", None, 0),
            DeleteMode::Recoverable { days: None },
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        out.recoverable_until.as_deref(),
        Some("2027-01-15T08:00:00Z")
    );
    assert!(matches!(
        b.delete(
            &at("app/db", None, 0),
            DeleteMode::Recoverable { days: Some(3) },
            None
        )
        .await
        .unwrap_err(),
        ExtError::Unsupported(_)
    ));
    let refused = b
        .delete(&at("app/db", None, 0), DeleteMode::Permanent, None)
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, ExtError::Denied(why) if why.contains("allow_permanent_delete")),
        "{refused:?}"
    );
    assert_eq!(permanent.num_calls(), 0);

    let mut c = cfg();
    c.allow_permanent_delete = true;
    let b = backend(c, &[&recoverable, &permanent]);
    let out = b
        .delete(&at("app/db", None, 0), DeleteMode::Permanent, None)
        .await
        .unwrap();
    assert_eq!(out.recoverable_until, None);
    assert_eq!(permanent.num_calls(), 1);
}

#[tokio::test]
async fn list_scopes_to_the_prefix_filters_the_query_and_caches_the_page() {
    let list = mock!(Client::list_secrets)
        .match_requests(|r| {
            r.filters().len() == 1
                && r.filters()[0].key() == Some(&FilterNameStringType::Name)
                && r.filters()[0].values() == ["app/"]
                && r.include_planned_deletion() == Some(false)
        })
        .then_output(|| {
            ListSecretsOutput::builder()
                .secret_list(
                    SecretListEntry::builder()
                        .name("app/db/main")
                        .set_secret_versions_to_stages(Some(stages(&[
                            ("v9", &["custom"]),
                            ("v1", &[PREVIOUS]),
                            ("v2", &[CURRENT]),
                        ])))
                        .last_changed_date(DateTime::from_secs(1_700_000_000))
                        .build(),
                )
                .secret_list(SecretListEntry::builder().name("app/ci/token").build())
                // The name filter matches words, not only prefixes: out of scope.
                .secret_list(SecretListEntry::builder().name("other/app/x").build())
                .next_token("page-2")
                .build()
        });
    let deleted = mock!(Client::list_secrets)
        .match_requests(|r| r.include_planned_deletion() == Some(true))
        .then_output(|| {
            ListSecretsOutput::builder()
                .secret_list(SecretListEntry::builder().name("app/live").build())
                .secret_list(
                    SecretListEntry::builder()
                        .name("app/old")
                        .deleted_date(DateTime::from_secs(1_700_000_000))
                        .build(),
                )
                .build()
        });
    let mut c = cfg();
    c.prefix = Some("app/".into());
    let b = backend(c, &[&list, &deleted]);

    let page = b.list(ListQuery::default()).await.unwrap();
    let names: Vec<_> = page.entries.iter().map(|e| e.r#ref.as_str()).collect();
    assert_eq!(names, ["aws/prod/app/db/main", "aws/prod/app/ci/token"]);
    assert_eq!(page.next.as_deref(), Some("page-2"));
    let db = &page.entries[0];
    assert_eq!(db.location, "eu-west-1 · aws/secretsmanager");
    let ids: Vec<_> = db.versions.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(ids, ["v2", "v1", "v9"]);
    assert_eq!(
        db.versions[0].changed.as_deref(),
        Some("2023-11-14T22:13:20Z")
    );
    assert_eq!(db.versions[1].changed, None);
    assert_eq!(db.recoverable_until, None);

    let page = b
        .list(ListQuery {
            query: Some("DB".into()),
            cursor: None,
            deleted: false,
        })
        .await
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    b.list(ListQuery::default()).await.unwrap();
    assert_eq!(
        list.num_calls(),
        2,
        "the unqueried page came from the cache"
    );

    let page = b
        .list(ListQuery {
            deleted: true,
            ..ListQuery::default()
        })
        .await
        .unwrap();
    let names: Vec<_> = page.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["app/old"], "only entries pending deletion");
    // At least the 7-day minimum window from the deletion.
    assert_eq!(
        page.entries[0].recoverable_until.as_deref(),
        Some("2023-11-21T22:13:20Z")
    );
}

#[derive(Debug)]
struct Failing(fn() -> CredentialsError);

impl ProvideCredentials for Failing {
    fn provide_credentials<'a>(&'a self) -> future::ProvideCredentials<'a>
    where
        Self: 'a,
    {
        future::ProvideCredentials::ready(Err((self.0)()))
    }
}

/// What `aws-config`'s SSO token provider fails with once the session is over.
#[derive(Debug)]
struct SsoTokenExpired;

impl std::fmt::Display for SsoTokenExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the SSO token has expired and cannot be refreshed")
    }
}

impl StdError for SsoTokenExpired {}

/// A client whose credentials chain fails. No mock interceptor: it answers before identity
/// resolution, so the failure would never surface. The HTTP client is never reached.
fn failing_backend(provider: Failing) -> AwsBackend {
    let mut c = cfg();
    c.profile = Some("work-admin".into());
    let client = Client::from_conf(
        aws_sdk_secretsmanager::Config::builder()
            .with_test_defaults_v2()
            .http_client(aws_smithy_mocks::create_mock_http_client())
            .credentials_provider(provider)
            .build(),
    );
    AwsBackend::with_client(c, client)
}

#[tokio::test]
async fn an_expired_sso_session_is_expired_with_the_login_hint() {
    let b = failing_backend(Failing(|| {
        CredentialsError::provider_error(SsoTokenExpired)
    }));
    let err = b.list(ListQuery::default()).await.unwrap_err();
    let hint = "run aws sso login --profile work-admin, then retry";
    assert_eq!(err, ExtError::Expired(hint.into()));
    assert_eq!(b.status(), ConnStatus::Expired { hint: hint.into() });
    assert_eq!(
        b.probe().await.unwrap(),
        ConnStatus::Expired { hint: hint.into() }
    );
}

#[tokio::test]
async fn no_credentials_is_logged_out() {
    let b = failing_backend(Failing(|| {
        CredentialsError::not_loaded("no providers in chain provided credentials")
    }));
    assert_eq!(
        b.describe(&at("app/db", None, 0)).await.unwrap_err(),
        ExtError::LoggedOut
    );
    assert_eq!(b.status(), ConnStatus::LoggedOut);
}

#[test]
fn config_validation_names_the_problem() {
    let profiles = aws_profiles::AwsProfiles {
        profiles: vec!["work-admin".into()],
        selected: None,
        default_profile: "default".into(),
    };
    let mut c = cfg();
    c.profile = Some("work-admin".into());
    assert_eq!(c.validate(&profiles), Ok(()));
    c.profile = Some("missing".into());
    assert!(c.validate(&profiles).unwrap_err().contains("missing"));
    let mut c = cfg();
    c.delete_recovery_days = 31;
    assert!(c.validate(&profiles).is_err());
    let mut c = AwsConnConfig::new("Prod", "eu-west-1");
    assert!(c.validate(&profiles).is_err());
    c.conn = "prod".into();
    c.region = String::new();
    assert!(c.validate(&profiles).is_err());
}

/// Against `localstack` (221 §13 B3):
/// `docker run --rm -p 4566:4566 localstack/localstack` and
/// `AWS_ENDPOINT_URL=http://localhost:4566 AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test
/// cargo test -p secrets-aws -- --ignored`.
#[tokio::test]
#[ignore = "needs localstack on AWS_ENDPOINT_URL"]
async fn localstack_round_trip() {
    let name = format!("theuth-test/{}", uuid::Uuid::new_v4());
    let b = AwsBackend::new(AwsConnConfig::new("prod", "us-east-1"));
    let loc = Location::Aws {
        name: name.clone(),
        kms_key_id: None,
        description: None,
        json_key: None,
    };
    let (r, v1) = b
        .create(
            &loc,
            Value::Json(vec![("user", "app"), ("password", "one")]),
        )
        .await
        .unwrap();
    assert_eq!(
        b.create(&loc, Value::Plain("x")).await.unwrap_err(),
        ExtError::Exists
    );
    let key = |k: &str| ExtRef {
        key: Some(k.into()),
        ..r.clone()
    };
    assert_eq!(b.get(&key("password")).await.unwrap().0.as_str(), "one");

    let v2 = b
        .update(&key("password"), Value::Plain("two"), Some(&v1))
        .await
        .unwrap();
    assert!(matches!(
        b.update(&key("password"), Value::Plain("three"), Some(&v1))
            .await
            .unwrap_err(),
        ExtError::Conflict { .. }
    ));
    let (now, got) = b.get(&key("password")).await.unwrap();
    assert_eq!((now.as_str(), got.id.as_str()), ("two", v2.id.as_str()));
    assert_eq!(b.get(&key("user")).await.unwrap().0.as_str(), "app");
    let previous = ExtRef {
        back: 1,
        ..key("password")
    };
    assert_eq!(b.get(&previous).await.unwrap().0.as_str(), "one");

    let out = b
        .delete(&r, DeleteMode::Recoverable { days: None }, None)
        .await
        .unwrap();
    assert!(out.recoverable_until.is_some());
    b.restore(&r).await.unwrap();
    assert!(matches!(
        b.delete(&r, DeleteMode::Permanent, None).await.unwrap_err(),
        ExtError::Denied(_)
    ));
    let mut c = AwsConnConfig::new("prod", "us-east-1");
    c.allow_permanent_delete = true;
    let b = AwsBackend::new(c);
    let out = b.delete(&r, DeleteMode::Permanent, None).await.unwrap();
    assert_eq!(out.recoverable_until, None);
}
