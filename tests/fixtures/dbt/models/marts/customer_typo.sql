{{ config(materialized='table') }}
-- A typo in a column of a real table is still reported
select c.id, c.frist_name, o.amount
from customers c
join {{ ref('stg_orders') }} o on o.customer_id = c.id
