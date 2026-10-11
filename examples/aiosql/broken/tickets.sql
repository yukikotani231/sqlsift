-- Mistakes sqlsift catches before aiosql or the database sees them.
-- This directory is not in sqlsift.toml's files.

-- name: list-tickets-by-assignee
select id, subjct, status
  from tickets
 where assignee = :assignee

-- name: reopen-ticket!
update tickets
   set status = 'reopened'
 where id = :ticket_id

-- name: open-ticket<!
insert into tickets (customer_id, subject)
values (:customer_id, :subject)
returning id
