-- name: get-customer-by-email^
-- Look up one customer by email address.
select id, email, name, plan, created_at
  from customers
 where lower(email) = lower(:email);

-- name: create-customer<!
insert into customers (email, name, plan)
values (:email, :name, :plan)
returning id, created_at;

-- name: count-customers-on-plan$
select count(*) from customers where plan = :plan;

-- name: upgrade-plan!
update customers set plan = :plan where id = :customer_id;
