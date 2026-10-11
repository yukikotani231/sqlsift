select
    id as customer_id,
    first_name,
    last_name,
    lower(email) as email,
    created_at
from {{ source('raw', 'customers') }}
