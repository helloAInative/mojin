-- 摸金小王子 mojin.db · 33 张核心表
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS stock_universe (
  code TEXT PRIMARY KEY,
  name TEXT NOT NULL DEFAULT '',
  market TEXT NOT NULL DEFAULT '',
  industry TEXT NOT NULL DEFAULT '',
  listed_at TEXT,
  status TEXT NOT NULL DEFAULT 'active',
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS stock_bar (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  code TEXT NOT NULL,
  period TEXT NOT NULL,
  ts TEXT NOT NULL,
  open REAL NOT NULL,
  high REAL NOT NULL,
  low REAL NOT NULL,
  close REAL NOT NULL,
  volume REAL NOT NULL DEFAULT 0,
  amount REAL NOT NULL DEFAULT 0,
  UNIQUE(code, period, ts)
);
CREATE INDEX IF NOT EXISTS idx_bar_code_period_ts ON stock_bar(code, period, ts);

CREATE TABLE IF NOT EXISTS stock_fundamental (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  code TEXT NOT NULL,
  as_of TEXT NOT NULL,
  roe REAL,
  gross_margin REAL,
  cashflow REAL,
  pe REAL,
  pb REAL,
  pe_percentile REAL,
  goodwill REAL,
  pledge_ratio REAL,
  unlock_near REAL,
  industry_rank REAL,
  raw_json TEXT,
  UNIQUE(code, as_of)
);

CREATE TABLE IF NOT EXISTS stock_fund_flow (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  code TEXT NOT NULL,
  as_of TEXT NOT NULL,
  super_net REAL,
  big_net REAL,
  mid_net REAL,
  small_net REAL,
  main_net_ratio REAL,
  margin_balance REAL,
  UNIQUE(code, as_of)
);

CREATE TABLE IF NOT EXISTS signal (
  id TEXT PRIMARY KEY,
  code TEXT NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  level TEXT NOT NULL,
  confidence REAL NOT NULL DEFAULT 0,
  title TEXT NOT NULL,
  body TEXT NOT NULL DEFAULT '',
  period TEXT NOT NULL DEFAULT '1d',
  price REAL NOT NULL DEFAULT 0,
  fired_at TEXT NOT NULL,
  strategy_id TEXT,
  version_id TEXT,
  meta_json TEXT
);
CREATE INDEX IF NOT EXISTS idx_signal_code_time ON signal(code, fired_at);

CREATE TABLE IF NOT EXISTS signal_factor_hit (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  signal_id TEXT NOT NULL,
  factor_key TEXT NOT NULL,
  factor_value REAL,
  weight REAL NOT NULL DEFAULT 1,
  detail TEXT NOT NULL DEFAULT '',
  FOREIGN KEY(signal_id) REFERENCES signal(id)
);

CREATE TABLE IF NOT EXISTS signal_performance (
  signal_id TEXT PRIMARY KEY,
  ret_1d REAL,
  ret_3d REAL,
  ret_5d REAL,
  ret_10d REAL,
  ret_20d REAL,
  labeled_at TEXT,
  FOREIGN KEY(signal_id) REFERENCES signal(id)
);

CREATE TABLE IF NOT EXISTS strategy (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  mode TEXT NOT NULL DEFAULT 'signal',
  config_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS strategy_version (
  id TEXT PRIMARY KEY,
  strategy_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  config_json TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE(strategy_id, version),
  FOREIGN KEY(strategy_id) REFERENCES strategy(id)
);

CREATE TABLE IF NOT EXISTS paper_account (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL DEFAULT '家庭模拟盘',
  cash REAL NOT NULL DEFAULT 100000,
  frozen REAL NOT NULL DEFAULT 0,
  equity REAL NOT NULL DEFAULT 100000,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS paper_position (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  code TEXT NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  qty REAL NOT NULL DEFAULT 0,
  available REAL NOT NULL DEFAULT 0,
  cost REAL NOT NULL DEFAULT 0,
  UNIQUE(account_id, code),
  FOREIGN KEY(account_id) REFERENCES paper_account(id)
);

CREATE TABLE IF NOT EXISTS paper_mark (
  account_id TEXT NOT NULL,
  code TEXT NOT NULL,
  price REAL NOT NULL CHECK (price > 0),
  source TEXT NOT NULL,
  quote_time TEXT NOT NULL DEFAULT '',
  fetched_at TEXT NOT NULL,
  PRIMARY KEY(account_id, code),
  FOREIGN KEY(account_id) REFERENCES paper_account(id)
);

CREATE TABLE IF NOT EXISTS paper_order (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL,
  code TEXT NOT NULL,
  side TEXT NOT NULL,
  qty REAL NOT NULL,
  price REAL NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  reason TEXT NOT NULL DEFAULT '',
  strategy_id TEXT,
  signal_id TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  FOREIGN KEY(account_id) REFERENCES paper_account(id)
);

CREATE TABLE IF NOT EXISTS paper_fill (
  id TEXT PRIMARY KEY,
  order_id TEXT NOT NULL,
  account_id TEXT NOT NULL,
  code TEXT NOT NULL,
  side TEXT NOT NULL,
  qty REAL NOT NULL,
  price REAL NOT NULL,
  commission REAL NOT NULL DEFAULT 0,
  stamp_tax REAL NOT NULL DEFAULT 0,
  transfer_fee REAL NOT NULL DEFAULT 0,
  slippage REAL NOT NULL DEFAULT 0,
  filled_at TEXT NOT NULL,
  FOREIGN KEY(order_id) REFERENCES paper_order(id)
);

CREATE TABLE IF NOT EXISTS paper_risk_plan (
  account_id TEXT NOT NULL,
  code TEXT NOT NULL,
  source_order_id TEXT NOT NULL,
  entry_price REAL NOT NULL CHECK (entry_price > 0),
  stop_price REAL NOT NULL CHECK (stop_price > 0),
  take_profit_price REAL NOT NULL CHECK (take_profit_price > stop_price),
  risk_budget_pct REAL NOT NULL DEFAULT 0.01,
  suggested_position_pct REAL NOT NULL DEFAULT 0,
  basis TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY(account_id, code),
  FOREIGN KEY(account_id) REFERENCES paper_account(id),
  FOREIGN KEY(source_order_id) REFERENCES paper_order(id)
);

CREATE TABLE IF NOT EXISTS paper_daily_pnl (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  as_of TEXT NOT NULL,
  equity REAL NOT NULL,
  cash REAL NOT NULL,
  pnl REAL NOT NULL DEFAULT 0,
  ret REAL NOT NULL DEFAULT 0,
  bench_ret REAL,
  UNIQUE(account_id, as_of),
  FOREIGN KEY(account_id) REFERENCES paper_account(id)
);

CREATE TABLE IF NOT EXISTS factor_stat (
  factor_key TEXT PRIMARY KEY,
  weight REAL NOT NULL DEFAULT 1,
  samples INTEGER NOT NULL DEFAULT 0,
  win_rate REAL,
  payoff REAL,
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS model_route_stat (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  signal_type TEXT NOT NULL,
  role TEXT NOT NULL,
  accuracy REAL,
  samples INTEGER NOT NULL DEFAULT 0,
  weight REAL NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE(signal_type, role)
);

CREATE TABLE IF NOT EXISTS calibration_log (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  before_json TEXT,
  after_json TEXT,
  note TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS circuit_breaker (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL,
  reason TEXT NOT NULL,
  drawdown REAL,
  active INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  cleared_at TEXT,
  FOREIGN KEY(account_id) REFERENCES paper_account(id)
);

CREATE TABLE IF NOT EXISTS ai_cache (
  id TEXT PRIMARY KEY,
  cache_key TEXT NOT NULL UNIQUE,
  code TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'full',
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  expires_at TEXT
);

CREATE TABLE IF NOT EXISTS audit_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  actor TEXT NOT NULL DEFAULT 'system',
  action TEXT NOT NULL,
  detail TEXT NOT NULL DEFAULT '',
  target TEXT NOT NULL DEFAULT '',
  payload_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS schema_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS backtest (
  id TEXT PRIMARY KEY,
  strategy_id TEXT NOT NULL,
  version_id TEXT NOT NULL,
  scope TEXT NOT NULL DEFAULT 'universe',
  start_at TEXT NOT NULL,
  end_at TEXT NOT NULL,
  initial_equity REAL NOT NULL DEFAULT 1000000,
  metrics_json TEXT NOT NULL DEFAULT '{}',
  status TEXT NOT NULL DEFAULT 'done',
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS calibration_record (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  module TEXT NOT NULL,
  metric TEXT NOT NULL,
  bucket TEXT NOT NULL,
  n INTEGER NOT NULL,
  pred REAL,
  actual REAL,
  as_of TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS ai_usage (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  model TEXT NOT NULL,
  tokens_in INTEGER NOT NULL DEFAULT 0,
  tokens_out INTEGER NOT NULL DEFAULT 0,
  latency_ms INTEGER NOT NULL DEFAULT 0,
  ok INTEGER NOT NULL DEFAULT 1,
  fallback TEXT NOT NULL DEFAULT '',
  task TEXT NOT NULL DEFAULT '',
  prompt_kind TEXT NOT NULL DEFAULT '',
  cost_estimate REAL NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS diary (
  day TEXT PRIMARY KEY,
  body TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS prompt_template (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  body TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  enabled INTEGER NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS notify_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,
  payload_json TEXT NOT NULL DEFAULT '{}',
  channel TEXT NOT NULL DEFAULT 'macos',
  delivered INTEGER NOT NULL DEFAULT 0,
  fired_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS health_snapshot (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  source TEXT NOT NULL,
  ok INTEGER NOT NULL,
  latency_ms INTEGER NOT NULL DEFAULT 0,
  fail_streak INTEGER NOT NULL DEFAULT 0,
  cache_age_ms INTEGER NOT NULL DEFAULT 0,
  payload_json TEXT NOT NULL DEFAULT '{}',
  captured_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS index_universe (
  code TEXT PRIMARY KEY,
  name TEXT NOT NULL DEFAULT '',
  enabled INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS index_bar (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  code TEXT NOT NULL,
  period TEXT NOT NULL,
  ts TEXT NOT NULL,
  open REAL NOT NULL,
  high REAL NOT NULL,
  low REAL NOT NULL,
  close REAL NOT NULL,
  volume REAL NOT NULL DEFAULT 0,
  UNIQUE(code, period, ts)
);

CREATE TABLE IF NOT EXISTS watchlist (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  group_name TEXT NOT NULL DEFAULT '默认',
  code TEXT NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  added_at TEXT NOT NULL DEFAULT (datetime('now')),
  note TEXT NOT NULL DEFAULT '',
  UNIQUE(group_name, code)
);

INSERT OR IGNORE INTO schema_meta(key, value) VALUES ('version', '2');
UPDATE schema_meta SET value = '4' WHERE key = 'version';
INSERT OR IGNORE INTO schema_meta(key, value) VALUES ('name', 'mojin-v4');
INSERT OR IGNORE INTO schema_meta(key, value) VALUES ('updated_at', datetime('now'));

INSERT OR IGNORE INTO paper_account(id, name, cash, frozen, equity)
VALUES ('default', '摸金小王子·默认', 100000, 0, 100000);

INSERT OR IGNORE INTO watchlist(group_name, code, name) VALUES('默认','sz300623','捷捷微电');

INSERT OR IGNORE INTO prompt_template(id, name, body, version) VALUES
('sys.v1','系统角色','你是摸金小王子的家庭自用 AI 投研助手。所有结论必须先讲依据，再讲观点。永远不构成投资建议。',1);

INSERT OR IGNORE INTO index_universe(code, name, enabled) VALUES
  ('sh000001','上证指数',1),
  ('sz399001','深证成指',1),
  ('sz399006','创业板指',1),
  ('sh000300','沪深300',1),
  ('bj899050','北证50',1);

INSERT INTO audit_log(actor, action, target, payload_json)
SELECT 'system','init','mojin-v4','{"tables":33,"version":"4"}'
WHERE NOT EXISTS (SELECT 1 FROM audit_log WHERE actor = 'system' AND action = 'init' AND target = 'mojin-v4');
