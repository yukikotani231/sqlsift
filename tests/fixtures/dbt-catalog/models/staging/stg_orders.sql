{{ config(materialized='view') }}

select
    id as order_id,
    customer_id,
    order_date,
    status,
    amount_cents / 100.0 as amount
from {{ source('shop', 'orders') }}
