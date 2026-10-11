{{ config(materialized='table') }}
-- `amount_usd` is not a column of stg_orders (target/catalog.json)
select o.order_id, o.amount_usd
from {{ ref('stg_orders') }} o
