use actix_web::{web, HttpRequest, HttpResponse};
use router_env::{instrument, tracing, Flow};

use super::AppState;
#[cfg(feature = "v2")]
use crate::types::domain;
use crate::{
    core::{api_locking, payments::helpers},
    services::{api, authentication as auth},
};

#[cfg(feature = "v1")]
#[instrument(skip_all, fields(flow = ?Flow::EphemeralKeyCreate))]
pub async fn ephemeral_key_create(
    state: web::Data<AppState>,
    req: HttpRequest,
    json_payload: web::Json<api_models::ephemeral_key::EphemeralKeyCreateRequest>,
) -> HttpResponse {
    let flow = Flow::EphemeralKeyCreate;
    let payload = json_payload.into_inner();
    api::server_wrap(
        flow,
        state,
        &req,
        payload,
        |state, auth: auth::AuthenticationData, payload, _| {
            helpers::make_ephemeral_key(
                state,
                payload.customer_id,
                auth.merchant_account.get_id().to_owned(),
            )
        },
        &auth::HeaderAuth(auth::ApiKeyAuth {
            is_connected_allowed: false,
            is_platform_allowed: false,
        }),
        api_locking::LockAction::NotApplicable,
    )
    .await
}

#[cfg(feature = "v1")]
#[instrument(skip_all, fields(flow = ?Flow::EphemeralKeyDelete))]
pub async fn ephemeral_key_delete(
    state: web::Data<AppState>,
    req: HttpRequest,
    path: web::Path<String>,
) -> HttpResponse {
    let flow = Flow::EphemeralKeyDelete;
    let payload = path.into_inner();
    api::server_wrap(
        flow,
        state,
        &req,
        payload,
        |state, _: auth::AuthenticationData, req, _| helpers::delete_ephemeral_key(state, req),
        &auth::HeaderAuth(auth::ApiKeyAuth {
            is_connected_allowed: false,
            is_platform_allowed: false,
        }),
        api_locking::LockAction::NotApplicable,
    )
    .await
}

#[cfg(feature = "v2")]
#[instrument(skip_all, fields(flow = ?Flow::EphemeralKeyCreate))]
pub async fn client_secret_create(
    state: web::Data<AppState>,
    req: HttpRequest,
    json_payload: web::Json<api_models::ephemeral_key::ClientSecretCreateRequest>,
) -> HttpResponse {
    let flow = Flow::EphemeralKeyCreate;
    let payload = json_payload.into_inner();
    Box::pin(api::server_wrap(
        flow,
        state,
        &req,
        payload,
        |state, auth: auth::AuthenticationData, payload, _| {
            let merchant_context = domain::MerchantContext::NormalMerchant(Box::new(
                domain::Context(auth.merchant_account, auth.key_store),
            ));
            helpers::make_client_secret(
                state,
                payload.resource_id.to_owned(),
                merchant_context,
                req.headers(),
            )
        },
        &auth::V2ApiKeyAuth {
            is_connected_allowed: false,
            is_platform_allowed: false,
        },
        api_locking::LockAction::NotApplicable,
    ))
    .await
}

#[cfg(feature = "v2")]
#[instrument(skip_all, fields(flow = ?Flow::EphemeralKeyDelete))]
pub async fn client_secret_delete(
    state: web::Data<AppState>,
    req: HttpRequest,
    path: web::Path<String>,
) -> HttpResponse {
    let flow = Flow::EphemeralKeyDelete;
    let payload = path.into_inner();
    Box::pin(api::server_wrap(
        flow,
        state,
        &req,
        payload,
        |state, _: auth::AuthenticationData, req, _| helpers::delete_client_secret(state, req),
        &auth::V2ApiKeyAuth {
            is_connected_allowed: false,
            is_platform_allowed: false,
        },
        api_locking::LockAction::NotApplicable,
    ))
    .await
}
