-- request 事件下线 (2026-09-16): 每 attempt 一条、前端无消费者、信息与 requests 表重复
DELETE FROM events WHERE kind = 'request';
