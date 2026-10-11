-- name: get-author-by-id^
-- Get one author by id.
select id, nmae from authors where id = :id;

-- name: list_posts(author)
select id, title from posts where author_id = :author.id and published = :published

-- name: count_posts$
select count(*) from posts where author_id = :author_id;

-- name: publish_post!
update posts set published = true where id = :id and titel = :title;

-- name: create_post<!
insert into posts (author_id, title) values (:author_id, :title) returning id;

-- name: create_posts*!
insert into posts (author_id, title) values (:author_id, :title);

-- name: create_schema#
create table if not exists audit (id serial primary key, note text);
