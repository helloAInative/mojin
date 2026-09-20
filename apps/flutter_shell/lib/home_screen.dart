/// 摸金小王子 · Flutter 端 · 复盘工作台（命令式 UI）
///
/// 视图切换：总览 / 账户 / 信号 / 后验 / AI / WS 日志
///
/// 这是一个简化的终端式 UI：不依赖 Flutter Material，
/// 便于在没有完整 SDK 环境的机器上观察 dart 编译情况。

library;

import 'dart:async';

import 'package:flutter/widgets.dart';

import 'mojin_api.dart';
import 'mojin_models.dart';

enum Tab { dashboard, account, signals, performance, ai, wslog }

class HomeScreen extends StatefulWidget {
  final String baseUrl;
  const HomeScreen({super.key, this.baseUrl = 'http://127.0.0.1:8787'});

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  late final MojinApi _api = MojinApi(baseUrl: widget.baseUrl);
  Tab _tab = Tab.dashboard;
  Healthz? _healthz;
  Account? _account;
  List<SignalRow> _signals = const [];
  Performance? _perf;
  final List<String> _wsLog = [];
  StreamSubscription? _wsSub;
  Timer? _refreshTimer;
  String? _error;

  @override
  void initState() {
    super.initState();
    _refresh();
    _startWs();
    _refreshTimer = Timer.periodic(const Duration(seconds: 15), (_) {
      if (_tab == Tab.dashboard || _tab == Tab.performance || _tab == Tab.ai) {
        _refresh();
      }
    });
  }

  @override
  void dispose() {
    _wsSub?.cancel();
    _refreshTimer?.cancel();
    _api.close();
    super.dispose();
  }

  Future<void> _refresh() async {
    setState(() => _error = null);
    try {
      final h = _api.health();
      final a = _api.account();
      final s = _api.signals();
      final p = _api.performance(days: 5);
      final results = await Future.wait([h, a, s, p]);
      setState(() {
        _healthz = results[0] as Healthz;
        _account = results[1] as Account;
        _signals = results[2] as List<SignalRow>;
        _perf = results[3] as Performance;
      });
    } catch (e) {
      setState(() => _error = '$e');
    }
  }

  void _startWs() {
    _wsSub?.cancel();
    _wsSub = _api.wsStream(['signal', 'fill', 'equity', 'healthz', 'ai']).listen(
      (ev) {
        setState(() {
          _wsLog.insert(0, '${ev.ts} ${ev.topic} ${ev.payload}');
          if (_wsLog.length > 100) _wsLog.removeLast();
        });
      },
      onError: (e) {
        setState(() {
          _wsLog.insert(0, 'WS ERROR: $e');
        });
      },
    );
  }

  String _pct(double? x) => x == null
      ? '—'
      : '${x >= 0 ? '+' : ''}${(x * 100).toStringAsFixed(2)}%';
  String _num(double x) => x.toStringAsFixed(2);

  @override
  Widget build(BuildContext context) {
    return Directionality(
      textDirection: TextDirection.ltr,
      child: DefaultTextStyle(
        style: const TextStyle(
          fontFamily: 'Menlo',
          fontSize: 12,
          color: Color(0xFFE6EDF3),
        ),
        child: Container(
          color: const Color(0xFF0D1117),
          padding: const EdgeInsets.all(12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _header(),
              const SizedBox(height: 6),
              _nav(),
              const SizedBox(height: 10),
              Expanded(child: _body()),
            ],
          ),
        ),
      ),
    );
  }

  Widget _header() {
    return Row(children: [
      Text('摸金小王子 · ${widget.baseUrl}',
              style: const TextStyle(
                  color: const Color(0xFFF0B429),
                  fontWeight: FontWeight.w600,
                  fontSize: 14)),
      const SizedBox(width: 16),
      Text('信号 ${_healthz?.signals ?? 0} · 后验 ${_healthz?.signalsLabeled ?? 0}',
          style: const TextStyle(color: Color(0xFF9AA7B4))),
    ]);
  }

  Widget _nav() {
    final items = {
      Tab.dashboard: '总览',
      Tab.account: '账户/持仓',
      Tab.signals: '信号',
      Tab.performance: '后验',
      Tab.ai: 'AI',
      Tab.wslog: 'WS 日志',
    };
    return Wrap(
      spacing: 6,
      children: items.entries.map((e) {
        final active = _tab == e.key;
        return GestureDetector(
          onTap: () => setState(() => _tab = e.key),
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
            decoration: BoxDecoration(
              color: active ? const Color(0xFF161B22) : const Color(0xFF0A0D12),
              border: Border.all(color: active ? const Color(0xFFF0B429) : const Color(0xFF30363D)),
              borderRadius: BorderRadius.circular(8),
            ),
            child: Text(e.value, style: TextStyle(color: active ? Colors : const Color(0xFF9AA7B4))),
          ),
        );
      }).toList(),
    );
  }

  Widget _body() {
    if (_error != null) {
      return Text('error: $_error', style: const TextStyle(color: Color(0xFFF85149)));
    }
    switch (_tab) {
      case Tab.dashboard:
        return _dashboard();
      case Tab.account:
        return _accountView();
      case Tab.signals:
        return _signalsView();
      case Tab.performance:
        return _perfView();
      case Tab.ai:
        return _aiView();
      case Tab.wslog:
        return _wsLogView();
    }
  }

  Widget _dashboard() {
    final h = _healthz;
    final a = _account;
    final p = _perf?.stats;
    if (h == null) return const Text('加载中…');
    return ListView(children: [
      _card('健康', [
        _kv('service', h.service),
        _kv('schema', h.version),
        _kv('tables', h.tables.toString()),
        _kv('signals', h.signals.toString()),
        _kv('signals_labeled', h.signalsLabeled.toString()),
        _kv('hit_rate_5d', _pct(h.hitRate5d)),
      ]),
      _card('账户', [
        _kv('cash', _num(a?.cash ?? 0)),
        _kv('frozen', _num(a?.frozen ?? 0)),
        _kv('equity', _num(a?.equity ?? 0)),
        _kv('market_value', _num(a?.marketValue ?? 0)),
        _kv('unpriced_positions', '${a?.unpricedPositions ?? 0}'),
      ]),
      _card('后验（5 日）', [
        _kv('samples', p?.samples.toString() ?? '—'),
        _kv('hit_rate', p?.hitRate == null ? '—' : _pct(p!.hitRate!)),
        _kv('avg', _pct(p?.avgReturn)),
        _kv('best', _pct(p?.best)),
        _kv('worst', _pct(p?.worst)),
      ]),
    ]);
  }

  Widget _accountView() {
    final a = _account;
    if (a == null) return const Text('加载中…');
    return ListView(children: [
      _card('账户', [
        _kv('cash', _num(a.cash)),
        _kv('frozen', _num(a.frozen)),
        _kv('equity', _num(a.equity)),
        _kv('market_value', _num(a.marketValue)),
        _kv('unpriced_positions', '${a.unpricedPositions}'),
      ]),
      _card('持仓', a.positions.isEmpty
          ? [const Text('暂无持仓', style: TextStyle(color: Color(0xFF9AA7B4)))]
          : a.positions
              .map((p) => Text(
                  '${p.code} ${p.name} qty=${p.qty.toStringAsFixed(0)} available=${p.available.toStringAsFixed(0)} 成本=${_num(p.cost)} 估值=${_num(p.marketValue)} 行情=${p.marketPrice == null ? "—" : _num(p.marketPrice!)} ${p.markSource ?? "按成本"} ${p.quoteTime ?? ""}',
                  style: const TextStyle(fontFamily: 'Menlo')))
              .toList()),
    ]);
  }

  Widget _signalsView() {
    if (_signals.isEmpty) return const Text('暂无');
    return ListView(children: [
      _card('最近信号', _signals
          .take(50)
          .map((s) => Text(
              '${s.firedAt}  ${s.code} ${s.name} [${s.level}]  conf=${(s.confidence * 100).toStringAsFixed(0)}%  price=${_num(s.price)}',
              style: const TextStyle(fontFamily: 'Menlo')))
          .toList()),
    ]);
  }

  Widget _perfView() {
    final p = _perf;
    if (p == null) return const Text('加载中…');
    final header = const Text('window\t samples\t hit  avg\tworst',
        style: TextStyle(color: Color(0xFF9AA7B4)));
    final rows = [header];
    for (final w in p.windows) {
      rows.add(Text('${w.windowDays}d\t ${w.samples}\t ${_pct(w.hitRate)}  ${_pct(w.avgReturn)}\t${_pct(w.worst)}',
          style: const TextStyle(fontFamily: 'Menlo')));
    }
    return _card('多窗口后验', rows);
  }

  Widget _aiView() {
    return ListView(children: [
      _card('AI', [
        const Text('Flutter 端仅展示 ws 推送；分析请用 Tauri 壳或 curl POST /api/v1/ai/analyze/{code}',
            style: TextStyle(color: Color(0xFF9AA7B4))),
      ]),
    ]);
  }

  Widget _wsLogView() {
    return _card('WS 日志', _wsLog.isEmpty
        ? [const Text('等待推送…', style: TextStyle(color: Color(0xFF9AA7B4)))]
        : _wsLog.map((l) => Text(l, style: const TextStyle(fontFamily: 'Menlo', fontSize: 11))).toList());
  }

  Widget _card(String title, List<Widget> rows) {
    return Container(
      margin: const EdgeInsets.only(bottom: 10),
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: const Color(0xFF161B22),
        border: Border.all(color: const Color(0xFF30363D)),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Text(title,
            style: const TextStyle(
                color: Color(0xFF9AA7B4), fontWeight: FontWeight.w600)),
        const SizedBox(height: 6),
        ...rows,
      ]),
    );
  }

  Widget _kv(String k, String v) {
    return Row(mainAxisAlignment: MainAxisAlignment.spaceBetween, children: [
      Text(k, style: const TextStyle(color: Color(0xFF9AA7B4))),
      Text(v, style: const TextStyle(fontFamily: 'Menlo')),
    ]);
  }
}

const Colors = Color(0xFFE6EDF3);
