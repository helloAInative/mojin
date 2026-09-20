/// 摸金小王子 · 实体类（与 mj-server 的 JSON 字段对齐）
///
/// 不依赖任何第三方；只用了 `int` / `double` / `String` / `List`。
library;

double _d(dynamic v) => v == null ? 0.0 : (v as num).toDouble();
int _i(dynamic v) => v == null ? 0 : (v as num).toInt();
String _s(dynamic v) => v == null ? '' : v.toString();
bool _bool(dynamic v) => v == true;

class Healthz {
  final bool ok;
  final String service;
  final String version;
  final String dbPath;
  final int tables;
  final double paperEquity;
  final int signals;
  final int signalsLabeled;
  final double? hitRate5d;
  final double? hitRate10d;
  final double? hitRate20d;
  final String ts;
  Healthz.fromJson(Map<String, dynamic> j)
      : ok = _bool(j['ok']),
        service = _s(j['service']),
        version = _s(j['version']),
        dbPath = _s(j['db_path']),
        tables = _i(j['tables']),
        paperEquity = _d(j['paper_equity']),
        signals = _i(j['signals']),
        signalsLabeled = _i(j['signals_labeled']),
        hitRate5d = j['hit_rate_5d'] == null ? null : _d(j['hit_rate_5d']),
        hitRate10d = j['hit_rate_10d'] == null ? null : _d(j['hit_rate_10d']),
        hitRate20d = j['hit_rate_20d'] == null ? null : _d(j['hit_rate_20d']),
        ts = _s(j['ts']);
}

class Position {
  final String code;
  final String name;
  final double qty;
  final double available;
  final double cost;
  final double avgCost;
  final double? marketPrice;
  final double marketValue;
  final String? markSource;
  final String? quoteTime;
  final String? markedAt;
  Position.fromJson(Map<String, dynamic> j)
      : code = _s(j['code']),
        name = _s(j['name']),
        qty = _d(j['qty']),
        available = _d(j['available']),
        cost = _d(j['cost']),
        avgCost = _d(j['avg_cost']),
        marketPrice = j['market_price'] == null ? null : _d(j['market_price']),
        marketValue = _d(j['market_value']),
        markSource = j['mark_source']?.toString(),
        quoteTime = j['quote_time']?.toString(),
        markedAt = j['marked_at']?.toString();
}

class Account {
  final String id;
  final String name;
  final double cash;
  final double frozen;
  final double equity;
  final double marketValue;
  final int unpricedPositions;
  final List<Position> positions;
  Account.fromJson(Map<String, dynamic> j)
      : id = _s(j['id']),
        name = _s(j['name']),
        cash = _d(j['cash']),
        frozen = _d(j['frozen']),
        equity = _d(j['equity']),
        marketValue = _d(j['market_value']),
        unpricedPositions = _i(j['unpriced_positions']),
        positions = ((j['positions'] as List?) ?? const [])
            .map((e) => Position.fromJson(Map<String, dynamic>.from(e as Map)))
            .toList();
}

class SignalRow {
  final String id;
  final String code;
  final String name;
  final String level;
  final double confidence;
  final String title;
  final double price;
  final String firedAt;
  SignalRow.fromJson(Map<String, dynamic> j)
      : id = _s(j['id']),
        code = _s(j['code']),
        name = _s(j['name']),
        level = _s(j['level']),
        confidence = _d(j['confidence']),
        title = _s(j['title']),
        price = _d(j['price']),
        firedAt = _s(j['fired_at']);
}

class PerfStats {
  final int windowDays;
  final int samples;
  final double? hitRate;
  final double? avgReturn;
  final double? best;
  final double? worst;
  PerfStats.fromJson(Map<String, dynamic> j)
      : windowDays = _i(j['window']),
        samples = _i(j['samples']),
        hitRate = j['hit_rate'] == null ? null : _d(j['hit_rate']),
        avgReturn = j['avg_return'] == null ? null : _d(j['avg_return']),
        best = j['best'] == null ? null : _d(j['best']),
        worst = j['worst'] == null ? null : _d(j['worst']);
}

class Performance {
  final PerfStats stats;
  final List<PerfStats> windows;
  Performance.fromJson(Map<String, dynamic> j)
      : stats = PerfStats.fromJson(Map<String, dynamic>.from(j['stats'] as Map)),
        windows = ((j['windows'] as List?) ?? const [])
            .map((e) => PerfStats.fromJson(Map<String, dynamic>.from(e as Map)))
            .toList();
}

/// WS 事件（与 mj-server WsEvent 对应）
class WsEvent {
  final String topic; // signal / fill / equity / healthz / ai
  final String ts;
  final dynamic payload;
  WsEvent(this.topic, this.ts, this.payload);
  static WsEvent? fromJsonString(String s) {
    try {
      // 这里不引入 dart:convert 之外的依赖；用简易解析。正式工程建议 `dart:convert`。
      return null;
    } catch (_) {
      return null;
    }
  }
}
