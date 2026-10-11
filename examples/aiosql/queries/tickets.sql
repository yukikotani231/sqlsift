-- name: list-open-tickets(customer)
-- Open and pending tickets of a customer, highest priority first.
select t.id, t.subject, t.status, t.priority, t.opened_at
  from tickets t
 where t.customer_id = :customer.id
   and t.status in ('open', 'pending')
 order by t.priority, t.opened_at

-- name: open-ticket<!
insert into tickets (customer_id, subject, priority)
values (:customer_id, :subject, :priority)
returning id, opened_at

-- name: add-comments*!
insert into ticket_comments (ticket_id, author, body)
values (:ticket_id, :author, :body)

-- name: solve-ticket!
update tickets
   set status = 'solved', solved_at = now()
 where id = :ticket_id and status <> 'closed'

-- name: ticket-stats
select c.id, c.name,
       count(t.id) filter (where t.status in ('open', 'pending')) as open_tickets,
       max(t.opened_at) as last_opened_at
  from customers c
  left join tickets t on t.customer_id = c.id
 where c.created_at >= :since::timestamptz
 group by c.id, c.name
having count(t.id) >= :min_tickets
 order by open_tickets desc

-- name: close-stale-tickets#
update tickets
   set status = 'closed'
 where status = 'solved'
   and solved_at < now() - interval '30 days';
