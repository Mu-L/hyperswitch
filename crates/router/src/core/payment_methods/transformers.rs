pub use ::payment_methods::controller::{DataDuplicationCheck, DeleteCardResp};
#[cfg(feature = "v2")]
use api_models::payment_methods::PaymentMethodResponseItem;
use api_models::{enums as api_enums, payment_methods::Card};
use common_utils::{
    ext_traits::{Encode, StringExt},
    id_type,
    pii::Email,
    request::RequestContent,
};
use error_stack::ResultExt;
#[cfg(feature = "v2")]
use hyperswitch_domain_models::payment_method_data;
use josekit::jwe;
use router_env::tracing_actix_web::RequestId;
use serde::{Deserialize, Serialize};

#[cfg(feature = "v2")]
use crate::types::{payment_methods as pm_types, transformers};
use crate::{
    configs::settings,
    core::errors::{self, CustomResult},
    headers,
    pii::{prelude::*, Secret},
    services::{api as services, encryption, EncryptionAlgorithm},
    types::{api, domain},
    utils::OptionExt,
};

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum StoreLockerReq {
    LockerCard(StoreCardReq),
    LockerGeneric(StoreGenericReq),
}

impl StoreLockerReq {
    pub fn update_requestor_card_reference(&mut self, card_reference: Option<String>) {
        match self {
            Self::LockerCard(c) => c.requestor_card_reference = card_reference,
            Self::LockerGeneric(_) => (),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StoreCardReq {
    pub merchant_id: id_type::MerchantId,
    pub merchant_customer_id: id_type::CustomerId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requestor_card_reference: Option<String>,
    pub card: Card,
    pub ttl: i64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StoreGenericReq {
    pub merchant_id: id_type::MerchantId,
    pub merchant_customer_id: id_type::CustomerId,
    #[serde(rename = "enc_card_data")]
    pub enc_data: String,
    pub ttl: i64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StoreCardResp {
    pub status: String,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
    pub payload: Option<StoreCardRespPayload>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StoreCardRespPayload {
    pub card_reference: String,
    pub duplication_check: Option<DataDuplicationCheck>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CardReqBody {
    pub merchant_id: id_type::MerchantId,
    pub merchant_customer_id: id_type::CustomerId,
    pub card_reference: String,
}

#[cfg(feature = "v2")]
#[derive(Debug, Deserialize, Serialize)]
pub struct CardReqBodyV2 {
    pub merchant_id: id_type::MerchantId,
    pub merchant_customer_id: String, // Not changing this as it might lead to api contract failure
    pub card_reference: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RetrieveCardResp {
    pub status: String,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
    pub payload: Option<RetrieveCardRespPayload>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RetrieveCardRespPayload {
    pub card: Option<Card>,
    pub enc_card_data: Option<Secret<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddCardRequest {
    pub card_number: cards::CardNumber,
    pub customer_id: id_type::CustomerId,
    pub card_exp_month: Secret<String>,
    pub card_exp_year: Secret<String>,
    pub merchant_id: id_type::MerchantId,
    pub email_address: Option<Email>,
    pub name_on_card: Option<Secret<String>>,
    pub nickname: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddCardResponse {
    pub card_id: String,
    pub external_id: String,
    pub card_fingerprint: Secret<String>,
    pub card_global_fingerprint: Secret<String>,
    #[serde(rename = "merchant_id")]
    pub merchant_id: Option<id_type::MerchantId>,
    pub card_number: Option<cards::CardNumber>,
    pub card_exp_year: Option<Secret<String>>,
    pub card_exp_month: Option<Secret<String>>,
    pub name_on_card: Option<Secret<String>>,
    pub nickname: Option<String>,
    pub customer_id: Option<id_type::CustomerId>,
    pub duplicate: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddPaymentMethodResponse {
    pub payment_method_id: String,
    pub external_id: String,
    #[serde(rename = "merchant_id")]
    pub merchant_id: Option<id_type::MerchantId>,
    pub nickname: Option<String>,
    pub customer_id: Option<id_type::CustomerId>,
    pub duplicate: Option<bool>,
    pub payment_method_data: Secret<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetPaymentMethodResponse {
    pub payment_method: AddPaymentMethodResponse,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetCardResponse {
    pub card: AddCardResponse,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetCard<'a> {
    merchant_id: &'a str,
    card_id: &'a str,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteCardResponse {
    pub card_id: Option<String>,
    pub external_id: Option<String>,
    pub card_isin: Option<Secret<String>>,
    pub status: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct PaymentMethodMetadata {
    pub payment_method_tokenization: std::collections::HashMap<String, String>,
}

pub fn get_dotted_jwe(jwe: encryption::JweBody) -> String {
    let header = jwe.header;
    let encryption_key = jwe.encrypted_key;
    let iv = jwe.iv;
    let encryption_payload = jwe.encrypted_payload;
    let tag = jwe.tag;
    format!("{header}.{encryption_key}.{iv}.{encryption_payload}.{tag}")
}

pub fn get_dotted_jws(jws: encryption::JwsBody) -> String {
    let header = jws.header;
    let payload = jws.payload;
    let signature = jws.signature;
    format!("{header}.{payload}.{signature}")
}

pub async fn get_decrypted_response_payload(
    jwekey: &settings::Jwekey,
    jwe_body: encryption::JweBody,
    locker_choice: Option<api_enums::LockerChoice>,
    decryption_scheme: settings::DecryptionScheme,
) -> CustomResult<String, errors::VaultError> {
    let target_locker = locker_choice.unwrap_or(api_enums::LockerChoice::HyperswitchCardVault);

    let public_key = match target_locker {
        api_enums::LockerChoice::HyperswitchCardVault => {
            jwekey.vault_encryption_key.peek().as_bytes()
        }
    };

    let private_key = jwekey.vault_private_key.peek().as_bytes();

    let jwt = get_dotted_jwe(jwe_body);
    let alg = match decryption_scheme {
        settings::DecryptionScheme::RsaOaep => jwe::RSA_OAEP,
        settings::DecryptionScheme::RsaOaep256 => jwe::RSA_OAEP_256,
    };

    let jwe_decrypted = encryption::decrypt_jwe(
        &jwt,
        encryption::KeyIdCheck::SkipKeyIdCheck,
        private_key,
        alg,
    )
    .await
    .change_context(errors::VaultError::SaveCardFailed)
    .attach_printable("Jwe Decryption failed for JweBody for vault")?;

    let jws = jwe_decrypted
        .parse_struct("JwsBody")
        .change_context(errors::VaultError::ResponseDeserializationFailed)?;
    let jws_body = get_dotted_jws(jws);

    encryption::verify_sign(jws_body, public_key)
        .change_context(errors::VaultError::SaveCardFailed)
        .attach_printable("Jws Decryption failed for JwsBody for vault")
}

pub async fn get_decrypted_vault_response_payload(
    jwekey: &settings::Jwekey,
    jwe_body: encryption::JweBody,
    decryption_scheme: settings::DecryptionScheme,
) -> CustomResult<String, errors::VaultError> {
    let public_key = jwekey.vault_encryption_key.peek().as_bytes();

    let private_key = jwekey.vault_private_key.peek().as_bytes();

    let jwt = get_dotted_jwe(jwe_body);
    let alg = match decryption_scheme {
        settings::DecryptionScheme::RsaOaep => jwe::RSA_OAEP,
        settings::DecryptionScheme::RsaOaep256 => jwe::RSA_OAEP_256,
    };

    let jwe_decrypted = encryption::decrypt_jwe(
        &jwt,
        encryption::KeyIdCheck::SkipKeyIdCheck,
        private_key,
        alg,
    )
    .await
    .change_context(errors::VaultError::SaveCardFailed)
    .attach_printable("Jwe Decryption failed for JweBody for vault")?;

    let jws = jwe_decrypted
        .parse_struct("JwsBody")
        .change_context(errors::VaultError::ResponseDeserializationFailed)?;
    let jws_body = get_dotted_jws(jws);

    encryption::verify_sign(jws_body, public_key)
        .change_context(errors::VaultError::SaveCardFailed)
        .attach_printable("Jws Decryption failed for JwsBody for vault")
}

#[cfg(feature = "v2")]
pub async fn create_jwe_body_for_vault(
    jwekey: &settings::Jwekey,
    jws: &str,
) -> CustomResult<encryption::JweBody, errors::VaultError> {
    let jws_payload: Vec<&str> = jws.split('.').collect();

    let generate_jws_body = |payload: Vec<&str>| -> Option<encryption::JwsBody> {
        Some(encryption::JwsBody {
            header: payload.first()?.to_string(),
            payload: payload.get(1)?.to_string(),
            signature: payload.get(2)?.to_string(),
        })
    };

    let jws_body =
        generate_jws_body(jws_payload).ok_or(errors::VaultError::RequestEncryptionFailed)?;

    let payload = jws_body
        .encode_to_vec()
        .change_context(errors::VaultError::RequestEncodingFailed)?;

    let public_key = jwekey.vault_encryption_key.peek().as_bytes();

    let jwe_encrypted =
        encryption::encrypt_jwe(&payload, public_key, EncryptionAlgorithm::A256GCM, None)
            .await
            .change_context(errors::VaultError::SaveCardFailed)
            .attach_printable("Error on jwe encrypt")?;
    let jwe_payload: Vec<&str> = jwe_encrypted.split('.').collect();

    let generate_jwe_body = |payload: Vec<&str>| -> Option<encryption::JweBody> {
        Some(encryption::JweBody {
            header: payload.first()?.to_string(),
            iv: payload.get(2)?.to_string(),
            encrypted_payload: payload.get(3)?.to_string(),
            tag: payload.get(4)?.to_string(),
            encrypted_key: payload.get(1)?.to_string(),
        })
    };

    let jwe_body =
        generate_jwe_body(jwe_payload).ok_or(errors::VaultError::RequestEncodingFailed)?;

    Ok(jwe_body)
}

pub async fn mk_basilisk_req(
    jwekey: &settings::Jwekey,
    jws: &str,
    locker_choice: api_enums::LockerChoice,
) -> CustomResult<encryption::JweBody, errors::VaultError> {
    let jws_payload: Vec<&str> = jws.split('.').collect();

    let generate_jws_body = |payload: Vec<&str>| -> Option<encryption::JwsBody> {
        Some(encryption::JwsBody {
            header: payload.first()?.to_string(),
            payload: payload.get(1)?.to_string(),
            signature: payload.get(2)?.to_string(),
        })
    };

    let jws_body = generate_jws_body(jws_payload).ok_or(errors::VaultError::SaveCardFailed)?;

    let payload = jws_body
        .encode_to_vec()
        .change_context(errors::VaultError::SaveCardFailed)?;

    let public_key = match locker_choice {
        api_enums::LockerChoice::HyperswitchCardVault => {
            jwekey.vault_encryption_key.peek().as_bytes()
        }
    };

    let jwe_encrypted =
        encryption::encrypt_jwe(&payload, public_key, EncryptionAlgorithm::A256GCM, None)
            .await
            .change_context(errors::VaultError::SaveCardFailed)
            .attach_printable("Error on jwe encrypt")?;
    let jwe_payload: Vec<&str> = jwe_encrypted.split('.').collect();

    let generate_jwe_body = |payload: Vec<&str>| -> Option<encryption::JweBody> {
        Some(encryption::JweBody {
            header: payload.first()?.to_string(),
            iv: payload.get(2)?.to_string(),
            encrypted_payload: payload.get(3)?.to_string(),
            tag: payload.get(4)?.to_string(),
            encrypted_key: payload.get(1)?.to_string(),
        })
    };

    let jwe_body = generate_jwe_body(jwe_payload).ok_or(errors::VaultError::SaveCardFailed)?;

    Ok(jwe_body)
}

pub async fn mk_add_locker_request_hs(
    jwekey: &settings::Jwekey,
    locker: &settings::Locker,
    payload: &StoreLockerReq,
    locker_choice: api_enums::LockerChoice,
    tenant_id: id_type::TenantId,
    request_id: Option<RequestId>,
) -> CustomResult<services::Request, errors::VaultError> {
    let payload = payload
        .encode_to_vec()
        .change_context(errors::VaultError::RequestEncodingFailed)?;

    let private_key = jwekey.vault_private_key.peek().as_bytes();

    let jws = encryption::jws_sign_payload(&payload, &locker.locker_signing_key_id, private_key)
        .await
        .change_context(errors::VaultError::RequestEncodingFailed)?;

    let jwe_payload = mk_basilisk_req(jwekey, &jws, locker_choice).await?;
    let mut url = match locker_choice {
        api_enums::LockerChoice::HyperswitchCardVault => locker.host.to_owned(),
    };
    url.push_str("/cards/add");
    let mut request = services::Request::new(services::Method::Post, &url);
    request.add_header(headers::CONTENT_TYPE, "application/json".into());
    request.add_header(
        headers::X_TENANT_ID,
        tenant_id.get_string_repr().to_owned().into(),
    );
    if let Some(req_id) = request_id {
        request.add_header(
            headers::X_REQUEST_ID,
            req_id.as_hyphenated().to_string().into(),
        );
    }
    request.set_body(RequestContent::Json(Box::new(jwe_payload)));
    Ok(request)
}

#[cfg(all(feature = "v1", feature = "payouts"))]
pub fn mk_add_bank_response_hs(
    bank: api::BankPayout,
    bank_reference: String,
    req: api::PaymentMethodCreate,
    merchant_id: &id_type::MerchantId,
) -> api::PaymentMethodResponse {
    api::PaymentMethodResponse {
        merchant_id: merchant_id.to_owned(),
        customer_id: req.customer_id,
        payment_method_id: bank_reference,
        payment_method: req.payment_method,
        payment_method_type: req.payment_method_type,
        bank_transfer: Some(bank),
        card: None,
        metadata: req.metadata,
        created: Some(common_utils::date_time::now()),
        recurring_enabled: Some(false),           // [#256]
        installment_payment_enabled: Some(false), // #[#256]
        payment_experience: Some(vec![api_models::enums::PaymentExperience::RedirectToUrl]),
        last_used_at: Some(common_utils::date_time::now()),
        client_secret: None,
    }
}

#[cfg(all(feature = "v2", feature = "payouts"))]
pub fn mk_add_bank_response_hs(
    _bank: api::BankPayout,
    _bank_reference: String,
    _req: api::PaymentMethodCreate,
    _merchant_id: &id_type::MerchantId,
) -> api::PaymentMethodResponse {
    todo!()
}

#[cfg(feature = "v1")]
pub fn mk_add_card_response_hs(
    card: api::CardDetail,
    card_reference: String,
    req: api::PaymentMethodCreate,
    merchant_id: &id_type::MerchantId,
) -> api::PaymentMethodResponse {
    let card_number = card.card_number.clone();
    let last4_digits = card_number.get_last4();
    let card_isin = card_number.get_card_isin();

    let card = api::CardDetailFromLocker {
        scheme: card
            .card_network
            .clone()
            .map(|card_network| card_network.to_string()),
        last4_digits: Some(last4_digits),
        issuer_country: card.card_issuing_country,
        card_number: Some(card.card_number.clone()),
        expiry_month: Some(card.card_exp_month.clone()),
        expiry_year: Some(card.card_exp_year.clone()),
        card_token: None,
        card_fingerprint: None,
        card_holder_name: card.card_holder_name.clone(),
        nick_name: card.nick_name.clone(),
        card_isin: Some(card_isin),
        card_issuer: card.card_issuer,
        card_network: card.card_network,
        card_type: card.card_type,
        saved_to_locker: true,
    };
    api::PaymentMethodResponse {
        merchant_id: merchant_id.to_owned(),
        customer_id: req.customer_id,
        payment_method_id: card_reference,
        payment_method: req.payment_method,
        payment_method_type: req.payment_method_type,
        #[cfg(feature = "payouts")]
        bank_transfer: None,
        card: Some(card),
        metadata: req.metadata,
        created: Some(common_utils::date_time::now()),
        recurring_enabled: Some(false),           // [#256]
        installment_payment_enabled: Some(false), // #[#256]
        payment_experience: Some(vec![api_models::enums::PaymentExperience::RedirectToUrl]),
        last_used_at: Some(common_utils::date_time::now()), // [#256]
        client_secret: req.client_secret,
    }
}

#[cfg(feature = "v2")]
pub fn mk_add_card_response_hs(
    card: api::CardDetail,
    card_reference: String,
    req: api::PaymentMethodCreate,
    merchant_id: &id_type::MerchantId,
) -> api::PaymentMethodResponse {
    todo!()
}

#[cfg(feature = "v2")]
pub fn generate_pm_vaulting_req_from_update_request(
    pm_create: domain::PaymentMethodVaultingData,
    pm_update: api::PaymentMethodUpdateData,
) -> domain::PaymentMethodVaultingData {
    match (pm_create, pm_update) {
        (
            domain::PaymentMethodVaultingData::Card(card_create),
            api::PaymentMethodUpdateData::Card(update_card),
        ) => domain::PaymentMethodVaultingData::Card(api::CardDetail {
            card_number: card_create.card_number,
            card_exp_month: card_create.card_exp_month,
            card_exp_year: card_create.card_exp_year,
            card_issuing_country: card_create.card_issuing_country,
            card_network: card_create.card_network,
            card_issuer: card_create.card_issuer,
            card_type: card_create.card_type,
            card_holder_name: update_card
                .card_holder_name
                .or(card_create.card_holder_name),
            nick_name: update_card.nick_name.or(card_create.nick_name),
            card_cvc: None,
        }),
        _ => todo!(), //todo! - since support for network tokenization is not added PaymentMethodUpdateData. should be handled later.
    }
}

#[cfg(feature = "v2")]
pub fn generate_payment_method_response(
    payment_method: &domain::PaymentMethod,
    single_use_token: &Option<payment_method_data::SingleUsePaymentMethodToken>,
) -> errors::RouterResult<api::PaymentMethodResponse> {
    let pmd = payment_method
        .payment_method_data
        .clone()
        .map(|data| data.into_inner())
        .and_then(|data| match data {
            api::PaymentMethodsData::Card(card) => {
                Some(api::PaymentMethodResponseData::Card(card.into()))
            }
            _ => None,
        });
    let mut connector_tokens = payment_method
        .connector_mandate_details
        .as_ref()
        .and_then(|connector_token_details| connector_token_details.payments.clone())
        .map(|payment_token_details| payment_token_details.0)
        .map(|payment_token_details| {
            payment_token_details
                .into_iter()
                .map(transformers::ForeignFrom::foreign_from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if let Some(token) = single_use_token {
        let connector_token_single_use = transformers::ForeignFrom::foreign_from(token);
        connector_tokens.push(connector_token_single_use);
    }
    let connector_tokens = if connector_tokens.is_empty() {
        None
    } else {
        Some(connector_tokens)
    };

    let network_token_pmd = payment_method
        .network_token_payment_method_data
        .clone()
        .map(|data| data.into_inner())
        .and_then(|data| match data {
            domain::PaymentMethodsData::NetworkToken(token) => {
                Some(api::NetworkTokenDetailsPaymentMethod::from(token))
            }
            _ => None,
        });

    let network_token = network_token_pmd.map(|pmd| api::NetworkTokenResponse {
        payment_method_data: pmd,
    });

    let resp = api::PaymentMethodResponse {
        merchant_id: payment_method.merchant_id.to_owned(),
        customer_id: payment_method.customer_id.to_owned(),
        id: payment_method.id.to_owned(),
        payment_method_type: payment_method.get_payment_method_type(),
        payment_method_subtype: payment_method.get_payment_method_subtype(),
        created: Some(payment_method.created_at),
        recurring_enabled: Some(false),
        last_used_at: Some(payment_method.last_used_at),
        payment_method_data: pmd,
        connector_tokens,
        network_token,
    };

    Ok(resp)
}

#[allow(clippy::too_many_arguments)]
pub async fn mk_get_card_request_hs(
    jwekey: &settings::Jwekey,
    locker: &settings::Locker,
    customer_id: &id_type::CustomerId,
    merchant_id: &id_type::MerchantId,
    card_reference: &str,
    locker_choice: Option<api_enums::LockerChoice>,
    tenant_id: id_type::TenantId,
    request_id: Option<RequestId>,
) -> CustomResult<services::Request, errors::VaultError> {
    let merchant_customer_id = customer_id.to_owned();
    let card_req_body = CardReqBody {
        merchant_id: merchant_id.to_owned(),
        merchant_customer_id,
        card_reference: card_reference.to_owned(),
    };
    let payload = card_req_body
        .encode_to_vec()
        .change_context(errors::VaultError::RequestEncodingFailed)?;

    let private_key = jwekey.vault_private_key.peek().as_bytes();

    let jws = encryption::jws_sign_payload(&payload, &locker.locker_signing_key_id, private_key)
        .await
        .change_context(errors::VaultError::RequestEncodingFailed)?;

    let target_locker = locker_choice.unwrap_or(api_enums::LockerChoice::HyperswitchCardVault);

    let jwe_payload = mk_basilisk_req(jwekey, &jws, target_locker).await?;
    let mut url = match target_locker {
        api_enums::LockerChoice::HyperswitchCardVault => locker.host.to_owned(),
    };
    url.push_str("/cards/retrieve");
    let mut request = services::Request::new(services::Method::Post, &url);
    request.add_header(headers::CONTENT_TYPE, "application/json".into());
    request.add_header(
        headers::X_TENANT_ID,
        tenant_id.get_string_repr().to_owned().into(),
    );
    if let Some(req_id) = request_id {
        request.add_header(
            headers::X_REQUEST_ID,
            req_id.as_hyphenated().to_string().into(),
        );
    }

    request.set_body(RequestContent::Json(Box::new(jwe_payload)));
    Ok(request)
}

pub fn mk_get_card_request(
    locker: &settings::Locker,
    locker_id: &'static str,
    card_id: &'static str,
) -> CustomResult<services::Request, errors::VaultError> {
    let get_card_req = GetCard {
        merchant_id: locker_id,
        card_id,
    };

    let mut url = locker.host.to_owned();
    url.push_str("/card/getCard");
    let mut request = services::Request::new(services::Method::Post, &url);
    request.set_body(RequestContent::FormUrlEncoded(Box::new(get_card_req)));
    Ok(request)
}

pub fn mk_get_card_response(card: GetCardResponse) -> errors::RouterResult<Card> {
    Ok(Card {
        card_number: card.card.card_number.get_required_value("card_number")?,
        name_on_card: card.card.name_on_card,
        card_exp_month: card
            .card
            .card_exp_month
            .get_required_value("card_exp_month")?,
        card_exp_year: card
            .card
            .card_exp_year
            .get_required_value("card_exp_year")?,
        card_brand: None,
        card_isin: None,
        nick_name: card.card.nickname,
    })
}

pub async fn mk_delete_card_request_hs(
    jwekey: &settings::Jwekey,
    locker: &settings::Locker,
    customer_id: &id_type::CustomerId,
    merchant_id: &id_type::MerchantId,
    card_reference: &str,
    tenant_id: id_type::TenantId,
    request_id: Option<RequestId>,
) -> CustomResult<services::Request, errors::VaultError> {
    let merchant_customer_id = customer_id.to_owned();
    let card_req_body = CardReqBody {
        merchant_id: merchant_id.to_owned(),
        merchant_customer_id,
        card_reference: card_reference.to_owned(),
    };
    let payload = card_req_body
        .encode_to_vec()
        .change_context(errors::VaultError::RequestEncodingFailed)?;

    let private_key = jwekey.vault_private_key.peek().as_bytes();

    let jws = encryption::jws_sign_payload(&payload, &locker.locker_signing_key_id, private_key)
        .await
        .change_context(errors::VaultError::RequestEncodingFailed)?;

    let jwe_payload =
        mk_basilisk_req(jwekey, &jws, api_enums::LockerChoice::HyperswitchCardVault).await?;

    let mut url = locker.host.to_owned();
    url.push_str("/cards/delete");
    let mut request = services::Request::new(services::Method::Post, &url);
    request.add_header(headers::CONTENT_TYPE, "application/json".into());
    request.add_header(
        headers::X_TENANT_ID,
        tenant_id.get_string_repr().to_owned().into(),
    );
    if let Some(req_id) = request_id {
        request.add_header(
            headers::X_REQUEST_ID,
            req_id.as_hyphenated().to_string().into(),
        );
    }

    request.set_body(RequestContent::Json(Box::new(jwe_payload)));
    Ok(request)
}

// Need to fix this once we start moving to v2 completion
#[cfg(feature = "v2")]
pub async fn mk_delete_card_request_hs_by_id(
    jwekey: &settings::Jwekey,
    locker: &settings::Locker,
    id: &String,
    merchant_id: &id_type::MerchantId,
    card_reference: &str,
    tenant_id: id_type::TenantId,
    request_id: Option<RequestId>,
) -> CustomResult<services::Request, errors::VaultError> {
    let merchant_customer_id = id.to_owned();
    let card_req_body = CardReqBodyV2 {
        merchant_id: merchant_id.to_owned(),
        merchant_customer_id,
        card_reference: card_reference.to_owned(),
    };
    let payload = card_req_body
        .encode_to_vec()
        .change_context(errors::VaultError::RequestEncodingFailed)?;

    let private_key = jwekey.vault_private_key.peek().as_bytes();

    let jws = encryption::jws_sign_payload(&payload, &locker.locker_signing_key_id, private_key)
        .await
        .change_context(errors::VaultError::RequestEncodingFailed)?;

    let jwe_payload =
        mk_basilisk_req(jwekey, &jws, api_enums::LockerChoice::HyperswitchCardVault).await?;

    let mut url = locker.host.to_owned();
    url.push_str("/cards/delete");
    let mut request = services::Request::new(services::Method::Post, &url);
    request.add_header(headers::CONTENT_TYPE, "application/json".into());
    request.add_header(
        headers::X_TENANT_ID,
        tenant_id.get_string_repr().to_owned().into(),
    );
    if let Some(req_id) = request_id {
        request.add_header(
            headers::X_REQUEST_ID,
            req_id.as_hyphenated().to_string().into(),
        );
    }

    request.set_body(RequestContent::Json(Box::new(jwe_payload)));
    Ok(request)
}

pub fn mk_delete_card_response(
    response: DeleteCardResponse,
) -> errors::RouterResult<DeleteCardResp> {
    Ok(DeleteCardResp {
        status: response.status,
        error_message: None,
        error_code: None,
    })
}

#[cfg(feature = "v1")]
pub fn get_card_detail(
    pm: &domain::PaymentMethod,
    response: Card,
) -> CustomResult<api::CardDetailFromLocker, errors::VaultError> {
    let card_number = response.card_number;
    let last4_digits = card_number.clone().get_last4();
    //fetch form card bin

    let card_detail = api::CardDetailFromLocker {
        scheme: pm.scheme.to_owned(),
        issuer_country: pm.issuer_country.clone(),
        last4_digits: Some(last4_digits),
        card_number: Some(card_number),
        expiry_month: Some(response.card_exp_month),
        expiry_year: Some(response.card_exp_year),
        card_token: None,
        card_fingerprint: None,
        card_holder_name: response.name_on_card,
        nick_name: response.nick_name.map(Secret::new),
        card_isin: None,
        card_issuer: None,
        card_network: None,
        card_type: None,
        saved_to_locker: true,
    };
    Ok(card_detail)
}

#[cfg(feature = "v2")]
pub fn get_card_detail(
    _pm: &domain::PaymentMethod,
    response: Card,
) -> CustomResult<api::CardDetailFromLocker, errors::VaultError> {
    let card_number = response.card_number;
    let last4_digits = card_number.clone().get_last4();
    //fetch form card bin

    let card_detail = api::CardDetailFromLocker {
        issuer_country: None,
        last4_digits: Some(last4_digits),
        card_number: Some(card_number),
        expiry_month: Some(response.card_exp_month),
        expiry_year: Some(response.card_exp_year),
        card_fingerprint: None,
        card_holder_name: response.name_on_card,
        nick_name: response.nick_name.map(Secret::new),
        card_isin: None,
        card_issuer: None,
        card_network: None,
        card_type: None,
        saved_to_locker: true,
    };
    Ok(card_detail)
}

//------------------------------------------------TokenizeService------------------------------------------------
pub fn mk_crud_locker_request(
    locker: &settings::Locker,
    path: &str,
    req: api::TokenizePayloadEncrypted,
    tenant_id: id_type::TenantId,
    request_id: Option<RequestId>,
) -> CustomResult<services::Request, errors::VaultError> {
    let mut url = locker.basilisk_host.to_owned();
    url.push_str(path);
    let mut request = services::Request::new(services::Method::Post, &url);
    request.add_default_headers();
    request.add_header(headers::CONTENT_TYPE, "application/json".into());
    request.add_header(
        headers::X_TENANT_ID,
        tenant_id.get_string_repr().to_owned().into(),
    );
    if let Some(req_id) = request_id {
        request.add_header(
            headers::X_REQUEST_ID,
            req_id.as_hyphenated().to_string().into(),
        );
    }

    request.set_body(RequestContent::Json(Box::new(req)));
    Ok(request)
}

pub fn mk_card_value1(
    card_number: cards::CardNumber,
    exp_year: String,
    exp_month: String,
    name_on_card: Option<String>,
    nickname: Option<String>,
    card_last_four: Option<String>,
    card_token: Option<String>,
) -> CustomResult<String, errors::VaultError> {
    let value1 = api::TokenizedCardValue1 {
        card_number: card_number.peek().clone(),
        exp_year,
        exp_month,
        name_on_card,
        nickname,
        card_last_four,
        card_token,
    };
    let value1_req = value1
        .encode_to_string_of_json()
        .change_context(errors::VaultError::FetchCardFailed)?;
    Ok(value1_req)
}

pub fn mk_card_value2(
    card_security_code: Option<String>,
    card_fingerprint: Option<String>,
    external_id: Option<String>,
    customer_id: Option<id_type::CustomerId>,
    payment_method_id: Option<String>,
) -> CustomResult<String, errors::VaultError> {
    let value2 = api::TokenizedCardValue2 {
        card_security_code,
        card_fingerprint,
        external_id,
        customer_id,
        payment_method_id,
    };
    let value2_req = value2
        .encode_to_string_of_json()
        .change_context(errors::VaultError::FetchCardFailed)?;
    Ok(value2_req)
}

#[cfg(feature = "v2")]
impl transformers::ForeignTryFrom<(domain::PaymentMethod, String)>
    for api::CustomerPaymentMethodResponseItem
{
    type Error = error_stack::Report<errors::ValidationError>;

    fn foreign_try_from(
        (item, payment_token): (domain::PaymentMethod, String),
    ) -> Result<Self, Self::Error> {
        // For payment methods that are active we should always have the payment method subtype
        let payment_method_subtype =
            item.payment_method_subtype
                .ok_or(errors::ValidationError::MissingRequiredField {
                    field_name: "payment_method_subtype".to_string(),
                })?;

        // For payment methods that are active we should always have the payment method type
        let payment_method_type =
            item.payment_method_type
                .ok_or(errors::ValidationError::MissingRequiredField {
                    field_name: "payment_method_type".to_string(),
                })?;

        let payment_method_data = item
            .payment_method_data
            .map(|payment_method_data| payment_method_data.into_inner())
            .map(|payment_method_data| match payment_method_data {
                api_models::payment_methods::PaymentMethodsData::Card(
                    card_details_payment_method,
                ) => {
                    let card_details = api::CardDetailFromLocker::from(card_details_payment_method);
                    api_models::payment_methods::PaymentMethodListData::Card(card_details)
                }
                api_models::payment_methods::PaymentMethodsData::BankDetails(..) => todo!(),
                api_models::payment_methods::PaymentMethodsData::WalletDetails(..) => {
                    todo!()
                }
            });

        let payment_method_billing = item
            .payment_method_billing_address
            .clone()
            .map(|billing| billing.into_inner())
            .map(From::from);

        // TODO: check how we can get this field
        let recurring_enabled = true;

        Ok(Self {
            id: item.id,
            customer_id: item.customer_id,
            payment_method_type,
            payment_method_subtype,
            created: item.created_at,
            last_used_at: item.last_used_at,
            recurring_enabled,
            payment_method_data,
            bank: None,
            requires_cvv: true,
            is_default: false,
            billing: payment_method_billing,
            payment_token,
        })
    }
}

#[cfg(feature = "v2")]
impl transformers::ForeignTryFrom<domain::PaymentMethod> for PaymentMethodResponseItem {
    type Error = error_stack::Report<errors::ValidationError>;

    fn foreign_try_from(item: domain::PaymentMethod) -> Result<Self, Self::Error> {
        // For payment methods that are active we should always have the payment method subtype
        let payment_method_subtype =
            item.payment_method_subtype
                .ok_or(errors::ValidationError::MissingRequiredField {
                    field_name: "payment_method_subtype".to_string(),
                })?;

        // For payment methods that are active we should always have the payment method type
        let payment_method_type =
            item.payment_method_type
                .ok_or(errors::ValidationError::MissingRequiredField {
                    field_name: "payment_method_type".to_string(),
                })?;

        let payment_method_data = item
            .payment_method_data
            .map(|payment_method_data| payment_method_data.into_inner())
            .map(|payment_method_data| match payment_method_data {
                api_models::payment_methods::PaymentMethodsData::Card(
                    card_details_payment_method,
                ) => {
                    let card_details = api::CardDetailFromLocker::from(card_details_payment_method);
                    api_models::payment_methods::PaymentMethodListData::Card(card_details)
                }
                api_models::payment_methods::PaymentMethodsData::BankDetails(..) => todo!(),
                api_models::payment_methods::PaymentMethodsData::WalletDetails(..) => {
                    todo!()
                }
            });

        let payment_method_billing = item
            .payment_method_billing_address
            .clone()
            .map(|billing| billing.into_inner())
            .map(From::from);

        let network_token_pmd = item
            .network_token_payment_method_data
            .clone()
            .map(|data| data.into_inner())
            .and_then(|data| match data {
                domain::PaymentMethodsData::NetworkToken(token) => {
                    Some(api::NetworkTokenDetailsPaymentMethod::from(token))
                }
                _ => None,
            });

        let network_token_resp = network_token_pmd.map(|pmd| api::NetworkTokenResponse {
            payment_method_data: pmd,
        });

        // TODO: check how we can get this field
        let recurring_enabled = Some(true);

        let psp_tokenization_enabled = item.connector_mandate_details.and_then(|details| {
            details.payments.map(|payments| {
                payments.values().any(|connector_token_reference| {
                    connector_token_reference.connector_token_status
                        == api_enums::ConnectorTokenStatus::Active
                })
            })
        });

        Ok(Self {
            id: item.id,
            customer_id: item.customer_id,
            payment_method_type,
            payment_method_subtype,
            created: item.created_at,
            last_used_at: item.last_used_at,
            recurring_enabled,
            payment_method_data,
            bank: None,
            requires_cvv: true,
            is_default: false,
            billing: payment_method_billing,
            network_tokenization: network_token_resp,
            psp_tokenization_enabled: psp_tokenization_enabled.unwrap_or(false),
        })
    }
}

#[cfg(feature = "v2")]
pub fn generate_payment_method_session_response(
    payment_method_session: hyperswitch_domain_models::payment_methods::PaymentMethodSession,
    client_secret: Secret<String>,
    associated_payment: Option<api_models::payments::PaymentsResponse>,
    tokenization_service_response: Option<api_models::tokenization::GenericTokenizationResponse>,
) -> api_models::payment_methods::PaymentMethodSessionResponse {
    let next_action = associated_payment
        .as_ref()
        .and_then(|payment| payment.next_action.clone());

    let authentication_details =
        associated_payment.map(
            |payment| api_models::payment_methods::AuthenticationDetails {
                status: payment.status,
                error: payment.error,
            },
        );

    let token_id = tokenization_service_response
        .as_ref()
        .map(|tokenization_service_response| tokenization_service_response.id.clone());

    api_models::payment_methods::PaymentMethodSessionResponse {
        id: payment_method_session.id,
        customer_id: payment_method_session.customer_id,
        billing: payment_method_session
            .billing
            .map(|address| address.into_inner())
            .map(From::from),
        psp_tokenization: payment_method_session.psp_tokenization,
        network_tokenization: payment_method_session.network_tokenization,
        tokenization_data: payment_method_session.tokenization_data,
        expires_at: payment_method_session.expires_at,
        client_secret,
        next_action,
        return_url: payment_method_session.return_url,
        associated_payment_methods: payment_method_session.associated_payment_methods,
        authentication_details,
        associated_token_id: token_id,
    }
}

#[cfg(feature = "v2")]
impl transformers::ForeignFrom<api_models::payment_methods::ConnectorTokenDetails>
    for hyperswitch_domain_models::mandates::ConnectorTokenReferenceRecord
{
    fn foreign_from(item: api_models::payment_methods::ConnectorTokenDetails) -> Self {
        let api_models::payment_methods::ConnectorTokenDetails {
            status,
            connector_token_request_reference_id,
            original_payment_authorized_amount,
            original_payment_authorized_currency,
            metadata,
            token,
            ..
        } = item;

        Self {
            connector_token: token.expose().clone(),
            // TODO: check why do we need this field
            payment_method_subtype: None,
            original_payment_authorized_amount,
            original_payment_authorized_currency,
            metadata,
            connector_token_status: status,
            connector_token_request_reference_id,
        }
    }
}

#[cfg(feature = "v2")]
impl
    transformers::ForeignFrom<(
        id_type::MerchantConnectorAccountId,
        hyperswitch_domain_models::mandates::ConnectorTokenReferenceRecord,
    )> for api_models::payment_methods::ConnectorTokenDetails
{
    fn foreign_from(
        (connector_id, mandate_reference_record): (
            id_type::MerchantConnectorAccountId,
            hyperswitch_domain_models::mandates::ConnectorTokenReferenceRecord,
        ),
    ) -> Self {
        let hyperswitch_domain_models::mandates::ConnectorTokenReferenceRecord {
            connector_token_request_reference_id,
            original_payment_authorized_amount,
            original_payment_authorized_currency,
            metadata,
            connector_token,
            connector_token_status,
            ..
        } = mandate_reference_record;

        Self {
            connector_id,
            status: connector_token_status,
            connector_token_request_reference_id,
            original_payment_authorized_amount,
            original_payment_authorized_currency,
            metadata,
            token: Secret::new(connector_token),
            // Token that is derived from payments mandate reference will always be multi use token
            token_type: common_enums::TokenizationType::MultiUse,
        }
    }
}

#[cfg(feature = "v2")]
impl transformers::ForeignFrom<&payment_method_data::SingleUsePaymentMethodToken>
    for api_models::payment_methods::ConnectorTokenDetails
{
    fn foreign_from(token: &payment_method_data::SingleUsePaymentMethodToken) -> Self {
        Self {
            connector_id: token.clone().merchant_connector_id,
            token_type: common_enums::TokenizationType::SingleUse,
            status: api_enums::ConnectorTokenStatus::Active,
            connector_token_request_reference_id: None,
            original_payment_authorized_amount: None,
            original_payment_authorized_currency: None,
            metadata: None,
            token: token.clone().token,
        }
    }
}
