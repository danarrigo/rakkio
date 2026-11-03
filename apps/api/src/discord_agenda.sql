-- One bounded entry per selected calendar event/series. Only sharing fields
-- are selected; event descriptions, locations and attendees never leave SQL.
WITH zones AS MATERIALIZED (SELECT name FROM pg_timezone_names), day AS (
    SELECT $3::date AS date,
        $3::date::timestamp AT TIME ZONE $2 AS starts,
        ($3::date + 1)::timestamp AT TIME ZONE $2 AS ends
), seeds AS (
    SELECT e.id, e.title, e.all_day, e.recurrence, e.recurrence_until,
        e.starts_at, e.ends_at, day.*,
        e.all_day AND e.starts_at AT TIME ZONE 'UTC' = date_trunc('day', e.starts_at AT TIME ZONE 'UTC')
            AND e.ends_at AT TIME ZONE 'UTC' = date_trunc('day', e.ends_at AT TIME ZONE 'UTC') AS floating,
        COALESCE(zones.name, $2) AS zone
    FROM calendar_events e
    JOIN calendars c ON c.id=e.calendar_id AND c.user_id=e.user_id
    LEFT JOIN zones ON zones.name=e.timezone
    CROSS JOIN day
    WHERE e.user_id=$1 AND c.selected
), local AS (
    SELECT *, starts_at AT TIME ZONE CASE WHEN floating THEN 'UTC' ELSE zone END AS seed_start,
        ends_at AT TIME ZONE CASE WHEN floating THEN 'UTC' ELSE zone END AS seed_end,
        CASE WHEN floating THEN date::timestamp AT TIME ZONE 'UTC' ELSE starts END AS span_start,
        CASE WHEN floating THEN (date+1)::timestamp AT TIME ZONE 'UTC' ELSE ends END AS span_end,
        CASE WHEN floating THEN 'UTC' ELSE zone END AS recurrence_zone
    FROM seeds
), indexed AS (
    SELECT *, GREATEST(0, CASE recurrence
        WHEN 'daily' THEN target::date-seed_start::date
        WHEN 'weekly' THEN (target::date-seed_start::date)/7
        WHEN 'monthly' THEN (extract(year FROM target)::int-extract(year FROM seed_start)::int)*12
            +extract(month FROM target)::int-extract(month FROM seed_start)::int
        WHEN 'yearly' THEN extract(year FROM target)::int-extract(year FROM seed_start)::int
        ELSE 0 END) AS period
    FROM local CROSS JOIN LATERAL (
        SELECT LEAST(span_end,COALESCE(recurrence_until,span_end)) AT TIME ZONE recurrence_zone AS target
    ) anchor
), shifted AS (
    SELECT indexed.*, seed_start+delta AS local_start, seed_end+delta AS local_end
    FROM indexed
    CROSS JOIN LATERAL generate_series(GREATEST(0,period-CASE WHEN recurrence='yearly' THEN 8 ELSE 2 END),period) n
    CROSS JOIN LATERAL (SELECT CASE recurrence
        WHEN 'daily' THEN make_interval(days=>n)
        WHEN 'weekly' THEN make_interval(days=>n*7)
        WHEN 'monthly' THEN make_interval(months=>n)
        WHEN 'yearly' THEN make_interval(years=>n)
        ELSE INTERVAL '0' END AS delta) step
    WHERE recurrence<>'none' OR n=0
), occurrences AS (
    SELECT *, local_start AT TIME ZONE recurrence_zone AS occurrence_start,
        local_end AT TIME ZONE recurrence_zone AS occurrence_end
    FROM shifted
    -- Skip missing month days/leap days instead of clamping them to another day.
    WHERE (recurrence NOT IN ('monthly','yearly') OR extract(day FROM local_start)=extract(day FROM seed_start))
      AND (recurrence<>'yearly' OR extract(month FROM local_start)=extract(month FROM seed_start))
      -- A missing local start during a DST jump is not another occurrence.
      AND (local_start AT TIME ZONE recurrence_zone) AT TIME ZONE recurrence_zone=local_start
), matching AS (
    SELECT DISTINCT ON (id) * FROM occurrences
    WHERE occurrence_start<span_end AND occurrence_end>span_start
      AND occurrence_end>occurrence_start
      AND (recurrence_until IS NULL OR occurrence_start<=recurrence_until)
    ORDER BY id,occurrence_start DESC
)
SELECT title, all_day,
    CASE WHEN (occurrence_start AT TIME ZONE $2)::date=date
        THEN to_char(occurrence_start AT TIME ZONE $2,'HH24:MI')
        ELSE to_char(occurrence_start AT TIME ZONE $2,'MM-DD HH24:MI') END AS start_label,
    CASE WHEN (occurrence_end AT TIME ZONE $2)::date=date
        THEN to_char(occurrence_end AT TIME ZONE $2,'HH24:MI')
        ELSE to_char(occurrence_end AT TIME ZONE $2,'MM-DD HH24:MI') END AS end_label
FROM matching
ORDER BY all_day DESC,occurrence_start,occurrence_end,id
LIMIT 11
