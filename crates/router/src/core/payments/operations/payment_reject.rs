use std::marker::PhantomData;

use api_models::{enums::FrmSuggestion, payments::PaymentsCancelRequest};
use async_trait::async_trait;
use error_stack::ResultExt;
use router_derive;
use router_env::{instrument, tracing};

use super::{BoxedOperation, Domain, GetTracker, Operation, UpdateTracker, ValidateRequest};
use crate::{
    core::{
        errors::{self, RouterResult, StorageErrorExt},
        payments::{helpers, operations, PaymentAddress, PaymentData},
    },
    events::audit_events::{AuditEvent, AuditEventType},
    routes::{app::ReqState, SessionState},
    services,
    types::{
        api::{self, PaymentIdTypeExt},
        domain,
        storage::{self, enums},
    },
    utils::OptionExt,
};

#[derive(Debug, Clone, Copy, router_derive::PaymentOperation)]
#[operation(operations = "all", flow = "cancel")]
pub struct PaymentReject;

type PaymentRejectOperation<'b, F> = BoxedOperation<'b, F, PaymentsCancelRequest, PaymentData<F>>;

#[async_trait]
impl<F: Send + Clone + Sync> GetTracker<F, PaymentData<F>, PaymentsCancelRequest>
    for PaymentReject
{
    #[instrument(skip_all)]
    async fn get_trackers<'a>(
        &'a self,
        state: &'a SessionState,
        payment_id: &api::PaymentIdType,
        _request: &PaymentsCancelRequest,
        merchant_context: &domain::MerchantContext,
        _auth_flow: services::AuthFlow,
        _header_payload: &hyperswitch_domain_models::payments::HeaderPayload,
    ) -> RouterResult<operations::GetTrackerResponse<'a, F, PaymentsCancelRequest, PaymentData<F>>>
    {
        let db = &*state.store;
        let key_manager_state = &state.into();

        let merchant_id = merchant_context.get_merchant_account().get_id();
        let storage_scheme = merchant_context.get_merchant_account().storage_scheme;
        let payment_id = payment_id
            .get_payment_intent_id()
            .change_context(errors::ApiErrorResponse::PaymentNotFound)?;

        let payment_intent = db
            .find_payment_intent_by_payment_id_merchant_id(
                key_manager_state,
                &payment_id,
                merchant_id,
                merchant_context.get_merchant_key_store(),
                storage_scheme,
            )
            .await
            .to_not_found_response(errors::ApiErrorResponse::PaymentNotFound)?;

        helpers::validate_payment_status_against_not_allowed_statuses(
            payment_intent.status,
            &[
                enums::IntentStatus::Cancelled,
                enums::IntentStatus::Failed,
                enums::IntentStatus::Succeeded,
                enums::IntentStatus::Processing,
            ],
            "reject",
        )?;

        let attempt_id = payment_intent.active_attempt.get_id().clone();
        let payment_attempt = db
            .find_payment_attempt_by_payment_id_merchant_id_attempt_id(
                &payment_intent.payment_id,
                merchant_id,
                attempt_id.clone().as_str(),
                storage_scheme,
            )
            .await
            .to_not_found_response(errors::ApiErrorResponse::PaymentNotFound)?;

        let shipping_address = helpers::get_address_by_id(
            state,
            payment_intent.shipping_address_id.clone(),
            merchant_context.get_merchant_key_store(),
            &payment_intent.payment_id,
            merchant_id,
            merchant_context.get_merchant_account().storage_scheme,
        )
        .await?;

        let billing_address = helpers::get_address_by_id(
            state,
            payment_intent.billing_address_id.clone(),
            merchant_context.get_merchant_key_store(),
            &payment_intent.payment_id,
            merchant_id,
            merchant_context.get_merchant_account().storage_scheme,
        )
        .await?;

        let payment_method_billing = helpers::get_address_by_id(
            state,
            payment_attempt.payment_method_billing_address_id.clone(),
            merchant_context.get_merchant_key_store(),
            &payment_intent.payment_id,
            merchant_id,
            merchant_context.get_merchant_account().storage_scheme,
        )
        .await?;

        let currency = payment_attempt.currency.get_required_value("currency")?;
        let amount = payment_attempt.get_total_amount().into();

        let frm_response = if cfg!(feature = "frm") {
            db.find_fraud_check_by_payment_id(payment_intent.payment_id.clone(), merchant_context.get_merchant_account().get_id().clone())
                .await
                .change_context(errors::ApiErrorResponse::PaymentNotFound)
                .attach_printable_lazy(|| {
                    format!("Error while retrieving frm_response, merchant_id: {:?}, payment_id: {attempt_id}", merchant_context.get_merchant_account().get_id())
                })
                .ok()
        } else {
            None
        };

        let profile_id = payment_intent
            .profile_id
            .as_ref()
            .get_required_value("profile_id")
            .change_context(errors::ApiErrorResponse::InternalServerError)
            .attach_printable("'profile_id' not set in payment intent")?;

        let business_profile = state
            .store
            .find_business_profile_by_profile_id(
                key_manager_state,
                merchant_context.get_merchant_key_store(),
                profile_id,
            )
            .await
            .to_not_found_response(errors::ApiErrorResponse::ProfileNotFound {
                id: profile_id.get_string_repr().to_owned(),
            })?;

        let payment_data = PaymentData {
            flow: PhantomData,
            payment_intent,
            payment_attempt,
            currency,
            amount,
            email: None,
            mandate_id: None,
            mandate_connector: None,
            setup_mandate: None,
            customer_acceptance: None,
            token: None,
            address: PaymentAddress::new(
                shipping_address.as_ref().map(From::from),
                billing_address.as_ref().map(From::from),
                payment_method_billing.as_ref().map(From::from),
                business_profile.use_billing_as_payment_method_billing,
            ),
            token_data: None,
            confirm: None,
            payment_method_data: None,
            payment_method_token: None,
            payment_method_info: None,
            force_sync: None,
            all_keys_required: None,
            refunds: vec![],
            disputes: vec![],
            attempts: None,
            sessions_token: vec![],
            card_cvc: None,
            creds_identifier: None,
            pm_token: None,
            connector_customer_id: None,
            recurring_mandate_payment_data: None,
            ephemeral_key: None,
            multiple_capture_data: None,
            redirect_response: None,
            surcharge_details: None,
            frm_message: frm_response,
            payment_link_data: None,
            incremental_authorization_details: None,
            authorizations: vec![],
            authentication: None,
            recurring_details: None,
            poll_config: None,
            tax_data: None,
            session_id: None,
            service_details: None,
            card_testing_guard_data: None,
            vault_operation: None,
            threeds_method_comp_ind: None,
            whole_connector_response: None,
        };

        let get_trackers_response = operations::GetTrackerResponse {
            operation: Box::new(self),
            customer_details: None,
            payment_data,
            business_profile,
            mandate_type: None,
        };

        Ok(get_trackers_response)
    }
}

#[async_trait]
impl<F: Clone + Sync> UpdateTracker<F, PaymentData<F>, PaymentsCancelRequest> for PaymentReject {
    #[instrument(skip_all)]
    async fn update_trackers<'b>(
        &'b self,
        state: &'b SessionState,
        req_state: ReqState,
        mut payment_data: PaymentData<F>,
        _customer: Option<domain::Customer>,
        storage_scheme: enums::MerchantStorageScheme,
        _updated_customer: Option<storage::CustomerUpdate>,
        key_store: &domain::MerchantKeyStore,
        _should_decline_transaction: Option<FrmSuggestion>,
        _header_payload: hyperswitch_domain_models::payments::HeaderPayload,
    ) -> RouterResult<(PaymentRejectOperation<'b, F>, PaymentData<F>)>
    where
        F: 'b + Send,
    {
        let intent_status_update = storage::PaymentIntentUpdate::RejectUpdate {
            status: enums::IntentStatus::Failed,
            merchant_decision: Some(enums::MerchantDecision::Rejected.to_string()),
            updated_by: storage_scheme.to_string(),
        };
        let (error_code, error_message) =
            payment_data
                .frm_message
                .clone()
                .map_or((None, None), |fraud_check| {
                    (
                        Some(Some(fraud_check.frm_status.to_string())),
                        Some(fraud_check.frm_reason.map(|reason| reason.to_string())),
                    )
                });
        let attempt_status_update = storage::PaymentAttemptUpdate::RejectUpdate {
            status: enums::AttemptStatus::Failure,
            error_code,
            error_message,
            updated_by: storage_scheme.to_string(),
        };

        payment_data.payment_intent = state
            .store
            .update_payment_intent(
                &state.into(),
                payment_data.payment_intent,
                intent_status_update,
                key_store,
                storage_scheme,
            )
            .await
            .to_not_found_response(errors::ApiErrorResponse::PaymentNotFound)?;

        payment_data.payment_attempt = state
            .store
            .update_payment_attempt_with_attempt_id(
                payment_data.payment_attempt.clone(),
                attempt_status_update,
                storage_scheme,
            )
            .await
            .to_not_found_response(errors::ApiErrorResponse::PaymentNotFound)?;
        let error_code = payment_data.payment_attempt.error_code.clone();
        let error_message = payment_data.payment_attempt.error_message.clone();
        req_state
            .event_context
            .event(AuditEvent::new(AuditEventType::PaymentReject {
                error_code,
                error_message,
            }))
            .with(payment_data.to_event())
            .emit();

        Ok((Box::new(self), payment_data))
    }
}

impl<F: Send + Clone + Sync> ValidateRequest<F, PaymentsCancelRequest, PaymentData<F>>
    for PaymentReject
{
    #[instrument(skip_all)]
    fn validate_request<'a, 'b>(
        &'b self,
        request: &PaymentsCancelRequest,
        merchant_context: &'a domain::MerchantContext,
    ) -> RouterResult<(PaymentRejectOperation<'b, F>, operations::ValidateResult)> {
        Ok((
            Box::new(self),
            operations::ValidateResult {
                merchant_id: merchant_context.get_merchant_account().get_id().to_owned(),
                payment_id: api::PaymentIdType::PaymentIntentId(request.payment_id.to_owned()),
                storage_scheme: merchant_context.get_merchant_account().storage_scheme,
                requeue: false,
            },
        ))
    }
}
