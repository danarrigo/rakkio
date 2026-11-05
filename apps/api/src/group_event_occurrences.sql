-- Bounded input: at most 25 authorized series, <=42 civil days, <=31-day
-- duration. Generate only nearby periods, never iterate from an old seed.
WITH bounds AS (
 SELECT $4::date AS first_day,$5::date AS last_day,
  $4::date::timestamp AT TIME ZONE $3 AS first_instant,
  $5::date::timestamp AT TIME ZONE $3 AS last_instant
), seeds AS (
 SELECT e.*,g.name AS group_name,(g.owner_id=$1 OR m.role='leader') AS can_edit,
  CASE WHEN e.all_day THEN first_day::timestamp ELSE first_instant AT TIME ZONE e.timezone END AS local_from,
  CASE WHEN e.all_day THEN last_day::timestamp ELSE last_instant AT TIME ZONE e.timezone END AS local_until,
  bounds.*
 FROM group_events e JOIN groups g ON g.id=e.group_id
 JOIN group_memberships m ON m.group_id=g.id AND m.user_id=$1 AND m.state='active'
 CROSS JOIN bounds
 WHERE e.id=ANY($2) AND e.deleted_at IS NULL AND g.deleted_at IS NULL
), indexed AS (
 SELECT *,CASE recurrence
   WHEN 'daily' THEN local_from::date-starts_local::date
   WHEN 'weekly' THEN (local_from::date-starts_local::date)/7
   WHEN 'monthly' THEN (extract(year FROM local_from)::int-extract(year FROM starts_local)::int)*12+extract(month FROM local_from)::int-extract(month FROM starts_local)::int
   WHEN 'yearly' THEN extract(year FROM local_from)::int-extract(year FROM starts_local)::int ELSE 0 END AS first_period,
  CASE recurrence
   WHEN 'daily' THEN local_until::date-starts_local::date
   WHEN 'weekly' THEN (local_until::date-starts_local::date)/7
   WHEN 'monthly' THEN (extract(year FROM local_until)::int-extract(year FROM starts_local)::int)*12+extract(month FROM local_until)::int-extract(month FROM starts_local)::int
   WHEN 'yearly' THEN extract(year FROM local_until)::int-extract(year FROM starts_local)::int ELSE 0 END AS last_period
 FROM seeds
), shifted AS (
 SELECT indexed.*,starts_local+delta AS local_start
 FROM indexed
 CROSS JOIN LATERAL generate_series(GREATEST(0,first_period-CASE recurrence WHEN 'daily' THEN 33 WHEN 'weekly' THEN 6 WHEN 'yearly' THEN 8 ELSE 2 END),GREATEST(0,last_period+1)) n
 CROSS JOIN LATERAL (SELECT CASE recurrence WHEN 'daily' THEN make_interval(days=>n) WHEN 'weekly' THEN make_interval(days=>n*7) WHEN 'monthly' THEN make_interval(months=>n) WHEN 'yearly' THEN make_interval(years=>n) ELSE INTERVAL '0' END AS delta) step
 WHERE recurrence<>'none' OR n=0
), civil AS (
 SELECT *,local_start+(ends_local-starts_local) AS local_end FROM shifted
 WHERE (recurrence NOT IN ('monthly','yearly') OR extract(day FROM local_start)=extract(day FROM starts_local))
  AND (recurrence<>'yearly' OR extract(month FROM local_start)=extract(month FROM starts_local))
  AND (recurrence_until IS NULL OR local_start::date<=recurrence_until)
), instants AS (
 SELECT *,local_start AT TIME ZONE CASE WHEN all_day THEN 'UTC' ELSE timezone END AS occurrence_start,
  local_end AT TIME ZONE CASE WHEN all_day THEN 'UTC' ELSE timezone END AS occurrence_end
 FROM civil
 WHERE all_day OR ((local_start AT TIME ZONE timezone) AT TIME ZONE timezone=local_start
  AND (local_end AT TIME ZONE timezone) AT TIME ZONE timezone=local_end)
)
SELECT 'group_event' AS origin,id AS event_id,group_id,group_name,can_edit,local_start::date AS original_date,title,
 occurrence_start AS starts_at,occurrence_end AS ends_at,all_day,timezone,version AS event_version
FROM instants
WHERE occurrence_end>occurrence_start AND (
 (all_day AND local_start<last_day::timestamp AND local_end>first_day::timestamp)
 OR (NOT all_day AND occurrence_start<last_instant AND occurrence_end>first_instant))
ORDER BY occurrence_start,occurrence_end,group_id,id
