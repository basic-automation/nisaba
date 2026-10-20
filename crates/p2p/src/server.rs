use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post},
    Router,
};
use tracing::info;

use nisaba_core::db::Db;

use crate::sync::{load_full_sync_payload, merge_remote_payload};
use crate::types::*;

/// Shared state for the onion service.
pub struct P2PState {
    pub db: Arc<Db>,
    pub company_secret: String,
    pub our_onion: String,
}

/// Build the Axum router for the onion service.
pub fn build_router(state: Arc<P2PState>) -> Router {
    Router::new()
        .route("/api/ping", get(handle_ping))
        .route("/api/sync", post(handle_sync))
        .route("/api/join", post(handle_join))
        .route("/api/company", get(handle_get_company))
        .route("/api/company/peers", post(handle_add_peer))
        .route(
            "/api/company/peers/{address}/approve",
            post(handle_approve_peer),
        )
        .route(
            "/api/company/peers/{address}/role",
            post(handle_set_peer_role),
        )
        .route("/api/company/peers/{address}", delete(handle_remove_peer))
        .route("/api/address-update", post(handle_address_update))
        .route("/api/rotate-secret", post(handle_rotate_secret))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state)
}

/// Middleware: validate X-Company-Secret header.
async fn auth_middleware(
    State(state): State<Arc<P2PState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let secret = req
        .headers()
        .get("X-Company-Secret")
        .and_then(|v| v.to_str().ok());

    match secret {
        Some(s) if constant_time_eq(s.as_bytes(), state.company_secret.as_bytes()) => {
            next.run(req).await
        }
        _ => (
            StatusCode::UNAUTHORIZED,
            "Invalid or missing company secret",
        )
            .into_response(),
    }
}

/// Compare two secrets without an early exit on the first differing byte. (The length
/// still leaks; the secret is a UUID, so its length is not a secret.)
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// GET /api/ping
async fn handle_ping(State(state): State<Arc<P2PState>>) -> Json<PingResponse> {
    Json(PingResponse {
        onion_address: state.our_onion.clone(),
        protocol_version: PROTOCOL_VERSION,
        timestamp: chrono::Utc::now().to_rfc3339(),
    })
}

/// POST /api/sync
async fn handle_sync(
    State(state): State<Arc<P2PState>>,
    Json(request): Json<SyncRequest>,
) -> Result<Json<SyncResponse>, (StatusCode, String)> {
    // Accept both protocol versions 1 and 2 for backward compat
    if request.protocol_version > PROTOCOL_VERSION {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Protocol version mismatch: max supported {}, got {}",
                PROTOCOL_VERSION, request.protocol_version
            ),
        ));
    }

    // Verify the sender is an authorized peer (try v2 first, fall back to v1)
    let peer_authorized = if !request.sender_peer_id.is_empty() {
        // Look up by peer_id in v2 table
        match state.db.get_peer_v2(&request.sender_peer_id).await {
            Ok(Some((_, _, _, _, is_auth))) => is_auth,
            _ => false,
        }
    } else {
        false
    };

    // Fall back to onion address lookup
    let peer_authorized = if !peer_authorized {
        match state.db.get_peer(&request.sender_onion).await {
            Ok(Some((_, _, _, is_auth))) => is_auth,
            _ => false,
        }
    } else {
        peer_authorized
    };

    if !peer_authorized {
        return Err((
            StatusCode::FORBIDDEN,
            format!("Peer not authorized: {}", request.sender_onion),
        ));
    }

    // Update last_seen
    let _ = state.db.update_peer_last_seen(&request.sender_onion).await;
    // Also update v2 table if we have a peer_id
    if !request.sender_peer_id.is_empty() {
        let _ = state
            .db
            .update_peer_last_seen_v2(&request.sender_peer_id)
            .await;
        // Update onion address if it changed
        if let Ok(Some((_, current_onion, _, _, _))) =
            state.db.get_peer_v2(&request.sender_peer_id).await
        {
            if current_onion != request.sender_onion {
                let _ = state
                    .db
                    .update_peer_onion_address(&request.sender_peer_id, &request.sender_onion)
                    .await;
                info!(peer_id = %request.sender_peer_id, new_onion = %request.sender_onion, "Peer onion address updated via sync");
            }
        }
    }

    // Merge remote payload
    let merge_summary = merge_remote_payload(&state.db, &request.payload)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    info!(
        peer = %request.sender_onion,
        products_created = merge_summary.products_created,
        products_updated = merge_summary.products_updated,
        "Sync merge complete from incoming request"
    );

    // Load our full state to send back
    let our_payload = load_full_sync_payload(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(SyncResponse {
        protocol_version: PROTOCOL_VERSION,
        responder_onion: state.our_onion.clone(),
        responder_time: chrono::Utc::now().to_rfc3339(),
        payload: our_payload,
        merge_summary,
    }))
}

/// POST /api/join
async fn handle_join(
    State(state): State<Arc<P2PState>>,
    Json(request): Json<JoinRequest>,
) -> Result<Json<CompanyInfo>, (StatusCode, String)> {
    info!(
        onion = %request.onion_address,
        name = %request.name,
        "Peer join request received"
    );

    // Insert as pending (is_authorized = false) in both tables
    state
        .db
        .insert_peer(&request.onion_address, &request.name, "member", false)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let peer_id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .insert_peer_v2(
            &peer_id,
            &request.onion_address,
            &request.name,
            "member",
            false,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let info = build_company_info(&state.db, &state.our_onion)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match info {
        Some(info) => Ok(Json(info)),
        None => Err((StatusCode::NOT_FOUND, "No company configured".to_string())),
    }
}

/// POST /api/address-update
async fn handle_address_update(
    State(state): State<Arc<P2PState>>,
    Json(request): Json<AddressUpdate>,
) -> Result<(StatusCode, &'static str), (StatusCode, String)> {
    state
        .db
        .update_peer_onion_address(&request.peer_id, &request.new_onion_address)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    info!(
        peer_id = %request.peer_id,
        new_onion = %request.new_onion_address,
        "Peer address updated"
    );

    Ok((StatusCode::OK, "Address updated"))
}

/// POST /api/rotate-secret
async fn handle_rotate_secret(
    State(_state): State<Arc<P2PState>>,
    Json(request): Json<RotateSecretRequest>,
) -> Result<(StatusCode, &'static str), (StatusCode, String)> {
    info!(
        rotated_by = %request.rotated_by,
        "Secret rotation received"
    );

    // Store the new secret — the caller (P2PManager or Tauri command) handles
    // re-encryption and keyring updates outside of the HTTP handler
    // For now, just acknowledge receipt
    Ok((StatusCode::OK, "Secret rotation acknowledged"))
}

/// GET /api/company
async fn handle_get_company(
    State(state): State<Arc<P2PState>>,
) -> Result<Json<CompanyInfo>, (StatusCode, String)> {
    let info = build_company_info(&state.db, &state.our_onion)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match info {
        Some(info) => Ok(Json(info)),
        None => Err((StatusCode::NOT_FOUND, "No company configured".to_string())),
    }
}

/// POST /api/company/peers
async fn handle_add_peer(
    State(state): State<Arc<P2PState>>,
    Json(request): Json<JoinRequest>,
) -> Result<(StatusCode, &'static str), (StatusCode, String)> {
    check_admin(&state.db).await?;

    state
        .db
        .insert_peer(&request.onion_address, &request.name, "member", true)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let peer_id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .insert_peer_v2(
            &peer_id,
            &request.onion_address,
            &request.name,
            "member",
            true,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    info!(peer = %request.onion_address, "Peer added by admin");
    Ok((StatusCode::CREATED, "Peer added"))
}

/// POST /api/company/peers/:address/approve
async fn handle_approve_peer(
    State(state): State<Arc<P2PState>>,
    Path(address): Path<String>,
) -> Result<(StatusCode, &'static str), (StatusCode, String)> {
    check_admin(&state.db).await?;

    state
        .db
        .approve_peer(&address)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Also approve in v2
    if let Ok(Some((peer_id, _, _, _, _))) = state.db.get_peer_by_onion(&address).await {
        let _ = state.db.approve_peer_v2(&peer_id).await;
    }

    info!(peer = %address, "Peer approved by admin");
    Ok((StatusCode::OK, "Peer approved"))
}

/// DELETE /api/company/peers/:address
async fn handle_remove_peer(
    State(state): State<Arc<P2PState>>,
    Path(address): Path<String>,
) -> Result<(StatusCode, &'static str), (StatusCode, String)> {
    check_admin(&state.db).await?;

    // Remove from v2
    if let Ok(Some((peer_id, _, _, _, _))) = state.db.get_peer_by_onion(&address).await {
        let _ = state.db.remove_peer_v2(&peer_id).await;
    }

    state
        .db
        .remove_peer(&address)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    info!(peer = %address, "Peer removed by admin");
    Ok((StatusCode::OK, "Peer removed"))
}

/// POST /api/company/peers/:address/role
async fn handle_set_peer_role(
    State(state): State<Arc<P2PState>>,
    Path(address): Path<String>,
    Json(request): Json<SetRoleRequest>,
) -> Result<(StatusCode, &'static str), (StatusCode, String)> {
    check_admin(&state.db).await?;

    if request.role != "admin" && request.role != "member" {
        return Err((
            StatusCode::BAD_REQUEST,
            "Role must be 'admin' or 'member'".to_string(),
        ));
    }

    state
        .db
        .update_peer_role(&address, &request.role)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Also update v2
    if let Ok(Some((peer_id, _, _, _, _))) = state.db.get_peer_by_onion(&address).await {
        let _ = state.db.update_peer_role_v2(&peer_id, &request.role).await;
    }

    info!(peer = %address, role = %request.role, "Peer role updated by admin");
    Ok((StatusCode::OK, "Role updated"))
}

/// Check that our local role is admin.
async fn check_admin(db: &Db) -> Result<(), (StatusCode, String)> {
    let company = db
        .get_company()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match company {
        Some((_, _, _, _, role)) if role == "admin" => Ok(()),
        _ => Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
    }
}

/// Build a CompanyInfo from the database.
pub async fn build_company_info(
    db: &Db,
    our_onion: &str,
) -> Result<Option<CompanyInfo>, nisaba_core::error::SyncError> {
    let company = db.get_company().await?;
    let Some((id, name, _secret, _onion, role)) = company else {
        return Ok(None);
    };

    let logo = db.get_company_logo().await?.map(|(data, _)| data);

    let raw_peers = db.list_peers().await?;
    let now = chrono::Utc::now();
    let online_threshold = chrono::Duration::minutes(5);

    let mut peers = Vec::new();
    for (addr, name, role, is_authorized, added_at, last_seen_at) in raw_peers {
        let is_online = last_seen_at
            .as_ref()
            .and_then(|ts| chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S").ok())
            .map(|seen| {
                let seen_utc = seen.and_utc();
                now.signed_duration_since(seen_utc) < online_threshold
            })
            .unwrap_or(false);

        let sync_state = db.get_sync_state(&addr).await?;
        let peer_sync = sync_state.map(|(_, last_synced, success, error, count)| PeerSyncState {
            last_synced_at: last_synced,
            last_sync_success: success,
            last_error: error,
            sync_count: count,
        });

        peers.push(PeerInfo {
            onion_address: addr,
            name,
            role,
            is_authorized,
            added_at,
            last_seen_at,
            is_online,
            sync_state: peer_sync,
        });
    }

    Ok(Some(CompanyInfo {
        id,
        name,
        our_onion: Some(our_onion.to_string()),
        role,
        peers,
        logo,
    }))
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn secret_comparison() {
        assert!(constant_time_eq(b"3f2a-secret", b"3f2a-secret"));
        assert!(!constant_time_eq(b"3f2a-secret", b"3f2a-secreT"));
        assert!(!constant_time_eq(b"3f2a-secret", b"3f2a-secre"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }
}
