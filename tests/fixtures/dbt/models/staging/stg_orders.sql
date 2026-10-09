{{ config(materialized='view') }}

{# Orders, with the amount in dollars #}
select
    id as order_id,
    customer_id,
    order_date,
    status,
    {{ cents_to_dollars('amount_cents') }} as amount
from {{ source('shop', 'orders') }}
{% if var('only_completed', false) %}
where status = 'completed'
{% endif %}
