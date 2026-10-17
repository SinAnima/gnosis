//! HTTPS client that accepts any server certificate.
//!
//! Used only when a scan passes `--s3-insecure` together with `--s3-endpoint`.
//! The default client keeps verifying TLS.

use aws_smithy_runtime_api::client::http::SharedHttpClient;
use aws_smithy_runtime_api::client::http::{HttpConnectorFuture, HttpConnectorSettings};
use aws_smithy_runtime_api::client::orchestrator::{HttpRequest, HttpResponse};
use aws_smithy_runtime_api::client::result::ConnectorError;
use aws_smithy_runtime_api::client::runtime_components::{
    RuntimeComponents, RuntimeComponentsBuilder,
};
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::config_bag::ConfigBag;
use bytes::Bytes;
use http_body_util::BodyExt;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error, SignatureScheme};
use std::sync::Arc;

pub(crate) fn insecure_client() -> SharedHttpClient {
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let schemes = provider
        .signature_verification_algorithms
        .supported_schemes();
    let config = rustls::ClientConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .expect("rustls protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert { schemes }))
        .with_no_client_auth();
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(config)
        .https_or_http()
        .enable_http1()
        .build();
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(connector);
    SharedHttpClient::new(InsecureHttp { client })
}

#[derive(Debug)]
struct AcceptAnyCert {
    schemes: Vec<SignatureScheme>,
}

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes.clone()
    }
}

#[derive(Clone, Debug)]
struct InsecureHttp {
    client: hyper_util::client::legacy::Client<
        hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
        http_body_util::Full<Bytes>,
    >,
}

impl aws_smithy_runtime_api::client::http::HttpClient for InsecureHttp {
    fn http_connector(
        &self,
        _settings: &HttpConnectorSettings,
        _components: &RuntimeComponents,
    ) -> aws_smithy_runtime_api::client::http::SharedHttpConnector {
        aws_smithy_runtime_api::client::http::SharedHttpConnector::new(self.clone())
    }

    fn validate_base_client_config(
        &self,
        _runtime_components: &RuntimeComponentsBuilder,
        _cfg: &ConfigBag,
    ) -> Result<(), aws_smithy_runtime_api::box_error::BoxError> {
        Ok(())
    }
}

impl aws_smithy_runtime_api::client::http::HttpConnector for InsecureHttp {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let client = self.client.clone();
        HttpConnectorFuture::new(async move {
            let hyper_req =
                smithy_to_hyper(request).map_err(|err| ConnectorError::other(err, None))?;
            let response = client
                .request(hyper_req)
                .await
                .map_err(|err| ConnectorError::other(err.into(), None))?;
            hyper_to_smithy(response)
                .await
                .map_err(|err| ConnectorError::other(err, None))
        })
    }
}

fn smithy_to_hyper(
    request: HttpRequest,
) -> Result<http::Request<http_body_util::Full<Bytes>>, Box<dyn std::error::Error + Send + Sync>> {
    let method = http::Method::from_bytes(request.method().as_bytes())?;
    let uri = request.uri().to_string();
    let mut builder = http::Request::builder().method(method).uri(uri);
    for (name, value) in request.headers().iter() {
        builder = builder.header(name, value);
    }
    let bytes = request
        .body()
        .bytes()
        .map(Bytes::copy_from_slice)
        .unwrap_or_default();
    Ok(builder.body(http_body_util::Full::new(bytes))?)
}

async fn hyper_to_smithy(
    response: http::Response<hyper::body::Incoming>,
) -> Result<HttpResponse, Box<dyn std::error::Error + Send + Sync>> {
    let (parts, body) = response.into_parts();
    let collected = body.collect().await?.to_bytes();
    let status = aws_smithy_runtime_api::http::StatusCode::try_from(parts.status.as_u16())?;
    let mut out = aws_smithy_runtime_api::http::Response::new(status, SdkBody::from(collected));
    for (name, value) in parts.headers.iter() {
        let name = name.as_str().to_string();
        let value = value.to_str().unwrap_or("").to_string();
        let _ = out.headers_mut().insert(name, value);
    }
    Ok(out)
}
