{{ config(materialized='table') }}

with orders as (
    select * from {{ ref('stg_orders') }}
)

select
    c.customer_id,
    c.first_name,
    count(o.order_id) as order_count,
    sum(o.amount) as lifetime_value,
    -- Not built yet (not in target/catalog.json): its columns are unknown
    max(n.anything) as newest
from {{ ref('stg_customers') }} c
left join orders o on o.customer_id = c.customer_id
left join {{ ref('not_built_yet') }} n on n.customer_id = c.customer_id
group by c.customer_id, c.first_name
