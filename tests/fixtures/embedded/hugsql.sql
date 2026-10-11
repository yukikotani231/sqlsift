-- :name get-author :? :1
-- :doc Get one author by id
select id, nmae from authors where id = :id

-- :name posts-by-ids :? :*
select id, title from posts where id in (:v*:ids) and author_id = :author-id

-- :name posts-sorted :? :*
select :i*:cols from posts order by :i:sort-col :sql:direction

-- :name insert-posts :! :n
insert into posts (author_id, title) values :t*:posts

-- :name posts-where :? :*
select id from posts :snip:where

-- :snip author-filter
where author_id = :author-id

-- :name posts-maybe-by-author :? :*
select id, nme from posts
--~ (when (:author-id params) "where author_id = :author-id")

-- :name dynamic-columns :? :*
select
/*~ (if (seq (:cols params)) */
:i*:cols
/*~*/
*
/*~ ) ~*/
from posts

-- :name update-title :! :n
update posts set titel = :title where id = :id::int
