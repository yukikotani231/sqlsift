-- Valid queries against webknossos schema

-- Query using ENUM columns
SELECT _id, name, state, typ FROM webknossos.annotations WHERE state = 'Active';

-- Query using CHECK constraint table
SELECT name, url, typ FROM webknossos.dataStores;

-- Query using VIEW
SELECT _id, name, isPublic FROM webknossos.annotations_;

-- Query joining table and view
SELECT a._id, a.name, t._id AS task_id
FROM webknossos.annotations a
JOIN webknossos.tasks t ON a._task = t._id
WHERE a.typ = 'Task';

-- Query the task_instances VIEW (derived from JOIN)
SELECT _id, assignedInstances, openInstances FROM webknossos.task_instances;

-- Query with subquery
SELECT _id, email FROM webknossos.users
WHERE _id IN (SELECT _user FROM webknossos.annotations WHERE typ = 'Explorational');
