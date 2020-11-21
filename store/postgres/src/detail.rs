//! Queries to support the index node API
use diesel::pg::PgConnection;
use diesel::prelude::{
    ExpressionMethods, JoinOnDsl, NullableExpressionMethods, OptionalExtension, QueryDsl,
    RunQueryDsl,
};
use graph::prelude::{bigdecimal::ToPrimitive, BigDecimal, StoreError};
use graph::{
    data::subgraph::{schema::SubgraphHealth, status},
    prelude::web3::types::H256,
};
use std::{convert::TryFrom, str::FromStr};

use crate::metadata::{subgraph, subgraph_version};

// This is not a real table, only a view. We can use diesel to read from it
// but write attempts will fail
table! {
    subgraphs.subgraph_deployment_detail (vid) {
        vid -> BigInt,
        id -> Text,
        manifest -> Text,
        failed -> Bool,
        health -> Text,
        synced -> Bool,
        fatal_error -> Nullable<Text>,
        non_fatal_errors -> Array<Text>,
        earliest_ethereum_block_hash -> Nullable<Binary>,
        earliest_ethereum_block_number -> Nullable<Numeric>,
        latest_ethereum_block_hash -> Nullable<Binary>,
        latest_ethereum_block_number -> Nullable<Numeric>,
        entity_count -> Numeric,
        graft_base -> Nullable<Text>,
        graft_block_hash -> Nullable<Binary>,
        graft_block_number -> Nullable<Numeric>,
        ethereum_head_block_hash -> Nullable<Binary>,
        ethereum_head_block_number -> Nullable<Numeric>,
        network -> Text,
        node_id -> Nullable<Text>,
        // We don't map block_range
        // block_range -> Range<Integer>,
    }
}

type Bytes = Vec<u8>;

#[derive(Queryable, QueryableByName)]
#[table_name = "subgraph_deployment_detail"]
// We map all fields to make loading `Detail` with diesel easier, but we
// don't need all the fields
#[allow(dead_code)]
struct Detail {
    vid: i64,
    id: String,
    manifest: String,
    failed: bool,
    health: String,
    synced: bool,
    fatal_error: Option<String>,
    non_fatal_errors: Vec<String>,
    earliest_ethereum_block_hash: Option<Bytes>,
    earliest_ethereum_block_number: Option<BigDecimal>,
    latest_ethereum_block_hash: Option<Bytes>,
    latest_ethereum_block_number: Option<BigDecimal>,
    entity_count: BigDecimal,
    graft_base: Option<String>,
    graft_block_hash: Option<Bytes>,
    graft_block_number: Option<BigDecimal>,
    ethereum_head_block_hash: Option<Bytes>,
    ethereum_head_block_number: Option<BigDecimal>,
    network: String,
    node_id: Option<String>,
}

impl TryFrom<Detail> for status::Info {
    type Error = StoreError;

    fn try_from(detail: Detail) -> Result<Self, Self::Error> {
        fn block(
            id: &str,
            name: &str,
            hash: Option<Vec<u8>>,
            number: Option<BigDecimal>,
        ) -> Result<Option<status::EthereumBlock>, StoreError> {
            match (&hash, &number) {
                (Some(hash), Some(number)) => {
                    let hash = H256::from_slice(hash.as_slice());
                    let number = number.to_u64().ok_or_else(|| {
                        StoreError::ConstraintViolation(format!(
                            "the block number {} for {} in {} is not representable as a u64",
                            number, name, id
                        ))
                    })?;
                    Ok(Some(status::EthereumBlock::new(hash, number)))
                }
                (None, None) => Ok(None),
                _ => Err(StoreError::ConstraintViolation(format!(
                    "the hash and number \
                of a block pointer must either both be null or both have a \
                value, but for `{}` the hash of {} is `{:?}` and the number is `{:?}`",
                    id, name, hash, number
                ))),
            }
        }

        let Detail {
            vid: _,
            id,
            manifest: _,
            failed: _,
            health,
            synced,
            fatal_error: _,
            non_fatal_errors: _,
            earliest_ethereum_block_hash,
            earliest_ethereum_block_number,
            latest_ethereum_block_hash,
            latest_ethereum_block_number,
            entity_count: _,
            graft_base: _,
            graft_block_hash: _,
            graft_block_number: _,
            ethereum_head_block_hash,
            ethereum_head_block_number,
            network,
            node_id,
        } = detail;

        let chain_head_block = block(
            &id,
            "ethereum_head_block",
            ethereum_head_block_hash,
            ethereum_head_block_number,
        )?;
        let earliest_block = block(
            &id,
            "earliest_ethereum_block",
            earliest_ethereum_block_hash,
            earliest_ethereum_block_number,
        )?;
        let latest_block = block(
            &id,
            "latest_ethereum_block",
            latest_ethereum_block_hash,
            latest_ethereum_block_number,
        )?;
        let health = SubgraphHealth::from_str(&health)?;
        let chain = status::ChainInfo {
            network,
            chain_head_block,
            earliest_block,
            latest_block,
        };
        Ok(status::Info {
            subgraph: id,
            synced,
            health,
            fatal_error: None,
            non_fatal_errors: vec![],
            chains: vec![chain],
            node: node_id,
        })
    }
}

pub(crate) fn deployments_for_subgraph(
    conn: &PgConnection,
    name: String,
) -> Result<Vec<String>, StoreError> {
    use subgraph as s;
    use subgraph_version as v;

    Ok(v::table
        .inner_join(s::table.on(v::subgraph.eq(s::id)))
        .filter(s::name.eq(&name))
        .order_by(v::created_at.asc())
        .select(v::deployment)
        .load(conn)?)
}

pub(crate) fn deployment_statuses(
    conn: &PgConnection,
    deployments: Vec<String>,
) -> Result<Vec<status::Info>, StoreError> {
    use subgraph_deployment_detail as d;

    // Empty deployments means 'all of them'
    if deployments.is_empty() {
        d::table
            .load::<Detail>(conn)?
            .into_iter()
            .map(|detail| status::Info::try_from(detail))
            .collect()
    } else {
        d::table
            .filter(d::id.eq_any(&deployments))
            .load::<Detail>(conn)?
            .into_iter()
            .map(|detail| status::Info::try_from(detail))
            .collect()
    }
}

pub fn subgraph_version(
    conn: &PgConnection,
    name: String,
    use_current: bool,
) -> Result<Option<String>, StoreError> {
    use subgraph as s;
    use subgraph_version as v;

    let deployment = if use_current {
        v::table
            .select(v::deployment.nullable())
            .inner_join(s::table.on(s::current_version.eq(v::id.nullable())))
            .filter(s::name.eq(&name))
            .first::<Option<String>>(conn)
    } else {
        v::table
            .select(v::deployment.nullable())
            .inner_join(s::table.on(s::pending_version.eq(v::id.nullable())))
            .filter(s::name.eq(&name))
            .first::<Option<String>>(conn)
    };
    Ok(deployment.optional()?.flatten())
}
