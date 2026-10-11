{% set methods = ['credit_card', 'coupon', 'bank_transfer', 'gift_card'] %}

select
    id as payment_id,
    order_id,
    payment_method,
    {% for method in methods %}
    case when payment_method = '{{ method }}' then amount else 0 end / 100.0 as {{ method }}_amount,
    {% endfor %}
    amount / 100.0 as amount
from {{ source('raw', 'payments') }}
