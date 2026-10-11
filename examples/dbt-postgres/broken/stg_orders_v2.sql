{{ config(materialized='view') }}

-- raw.orders calls the customer `user_id`, and order_date is a date.
select
    id as order_id,
    customer_id,
    order_date,
    status
from {{ source('raw', 'orders') }}
where order_date >= 20260101
