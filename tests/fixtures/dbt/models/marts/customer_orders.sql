{{
    config(
        materialized='incremental',
        unique_key='customer_id'
    )
}}

with orders as (
    select * from {{ ref('stg_orders') }}
)

select
    c.id as customer_id,
    c.first_name,
    c.last_name,
    count(o.order_id) as order_count,
    min(o.order_date) as first_order,
    {% for status in ['placed', 'shipped', 'completed'] %}
    sum(case when o.status = '{{ status }}' then 1 else 0 end) as {{ status }}_orders,
    {% endfor %}
    sum(o.amount) as lifetime_value
from customers c
left join orders o on o.customer_id = c.id
{% if is_incremental() %}
where c.id > (select max(customer_id) from {{ this }})
{% endif %}
group by c.id, c.first_name, c.last_name
