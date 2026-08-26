//! Durable payment-hash claims for settled BOLT12 zap events.

use buzz_core::{CommunityId, StoredEvent};
use nostr::Event;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::event::insert_event_with_thread_metadata_tx;
use crate::Result;

/// Result of atomically claiming a BOLT12 payment and storing its zap event.
#[derive(Debug)]
pub enum Bolt12ZapInsertOutcome {
    /// The payment claim and event were inserted.
    Inserted(Box<StoredEvent>),
    /// This exact event was already stored or claimed.
    EventDuplicate,
    /// Another event already claimed the payment hash.
    PaymentDuplicate,
}

/// Claim `payment_hash` and store `event` in one transaction.
///
/// Claim insertion comes first. A conflict returns before event insertion, so
/// two event envelopes for one settled payment cannot both enter the event
/// store. The claim remains community-scoped to preserve tenant isolation.
pub async fn insert_event(
    pool: &PgPool,
    community_id: CommunityId,
    event: &Event,
    channel_id: Option<Uuid>,
    payment_hash: &[u8; 32],
) -> Result<Bolt12ZapInsertOutcome> {
    let mut tx = pool.begin().await?;
    let event_id = event.id.as_bytes();
    let claim = sqlx::query(
        "INSERT INTO bolt12_zap_payments (community_id, payment_hash, event_id) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (community_id, payment_hash) DO NOTHING",
    )
    .bind(community_id.as_uuid())
    .bind(payment_hash.as_slice())
    .bind(event_id.as_slice())
    .execute(&mut *tx)
    .await?;

    if claim.rows_affected() == 0 {
        let claimed_event_id: Vec<u8> = sqlx::query(
            "SELECT event_id FROM bolt12_zap_payments \
             WHERE community_id = $1 AND payment_hash = $2",
        )
        .bind(community_id.as_uuid())
        .bind(payment_hash.as_slice())
        .fetch_one(&mut *tx)
        .await?
        .try_get("event_id")?;
        tx.rollback().await?;
        return Ok(if claimed_event_id == event_id.as_slice() {
            Bolt12ZapInsertOutcome::EventDuplicate
        } else {
            Bolt12ZapInsertOutcome::PaymentDuplicate
        });
    }

    let (stored_event, was_inserted) =
        insert_event_with_thread_metadata_tx(&mut tx, community_id, event, channel_id, None)
            .await?;
    tx.commit().await?;

    Ok(if was_inserted {
        Bolt12ZapInsertOutcome::Inserted(Box::new(stored_event))
    } else {
        Bolt12ZapInsertOutcome::EventDuplicate
    })
}

#[cfg(test)]
mod tests {
    use buzz_core::kind::KIND_BOLT12_ZAP;
    use nostr::{EventBuilder, Keys, Kind};
    use sqlx::PgPool;

    use super::*;

    const TEST_DB_URL: &str = "postgres://buzz:buzz_dev@localhost:5432/buzz"; // sadscan:disable np.postgres.1

    async fn setup_pool() -> PgPool {
        let database_url = std::env::var("BUZZ_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| TEST_DB_URL.to_owned());
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect test DB");
        crate::migration::run_migrations(&pool)
            .await
            .expect("run migrations");
        pool
    }

    async fn make_community(pool: &PgPool) -> CommunityId {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(id)
            .bind(format!("bolt12-zap-{}.example", id.simple()))
            .execute(pool)
            .await
            .expect("insert community");
        CommunityId::from_uuid(id)
    }

    fn zap(content: &str) -> Event {
        EventBuilder::new(Kind::Custom(KIND_BOLT12_ZAP as u16), content)
            .sign_with_keys(&Keys::generate())
            .expect("sign zap")
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn payment_hash_claim_is_atomic_and_community_scoped() {
        let pool = setup_pool().await;
        let community_a = make_community(&pool).await;
        let community_b = make_community(&pool).await;
        let payment_hash = [42_u8; 32];
        let first = zap("first");
        let second = zap("second");

        assert!(matches!(
            insert_event(&pool, community_a, &first, None, &payment_hash)
                .await
                .expect("insert first zap"),
            Bolt12ZapInsertOutcome::Inserted(_)
        ));
        assert!(matches!(
            insert_event(&pool, community_a, &first, None, &payment_hash)
                .await
                .expect("retry first zap"),
            Bolt12ZapInsertOutcome::EventDuplicate
        ));
        assert!(matches!(
            insert_event(&pool, community_a, &second, None, &payment_hash)
                .await
                .expect("reject duplicate payment"),
            Bolt12ZapInsertOutcome::PaymentDuplicate
        ));
        assert!(matches!(
            insert_event(&pool, community_b, &second, None, &payment_hash)
                .await
                .expect("insert in second community"),
            Bolt12ZapInsertOutcome::Inserted(_)
        ));

        let second_in_a = crate::event::get_event_by_id(&pool, community_a, second.id.as_bytes())
            .await
            .expect("query second zap");
        assert!(second_in_a.is_none());

        let racing_hash = [43_u8; 32];
        let racing_a = zap("racing a");
        let racing_b = zap("racing b");
        let (outcome_a, outcome_b) = tokio::join!(
            insert_event(&pool, community_a, &racing_a, None, &racing_hash),
            insert_event(&pool, community_a, &racing_b, None, &racing_hash),
        );
        let outcomes = [
            outcome_a.expect("first racing result"),
            outcome_b.expect("second racing result"),
        ];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Bolt12ZapInsertOutcome::Inserted(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Bolt12ZapInsertOutcome::PaymentDuplicate))
                .count(),
            1
        );
    }
}
