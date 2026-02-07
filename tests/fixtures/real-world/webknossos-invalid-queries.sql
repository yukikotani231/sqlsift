-- Invalid queries that should be caught by sqlsurge

-- Typo in column name
SELECT _id, nme FROM webknossos.annotations;

-- Non-existent table
SELECT * FROM webknossos.nonexistent_table;

-- Column from base table used on VIEW that only has inferred columns
SELECT nonexistent_col FROM webknossos.annotations_;

-- Wrong column name in JOIN condition
SELECT a._id FROM webknossos.annotations a JOIN webknossos.tasks t ON a.task_id = t._id;
