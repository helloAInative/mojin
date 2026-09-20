/// 摸金小王子 · 后端对接
///
/// 仅依赖 `dart:convert` + `package:http/http.dart` + `dart:io`（WS）。
///
/// 使用：
/// ```dart
/// final api = MojinApi(baseUrl: 'http://192.168.1.10:8787');
/// final h = await api.health();
/// ```
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;

import 'mojin_models.dart';

class MojinApi {
  final String baseUrl;
  final http.Client _http;
  final Duration _timeout;

  MojinApi({required this.baseUrl, http.Client? client, Duration? timeout})
      : _http = client ?? http.Client(),
        _timeout = timeout ?? const Duration(seconds: 8);

  Uri _u(String p) => Uri.parse('$baseUrl$p');

  Future<Map<String, dynamic>> _get(String path) async {
    final r = await _http.get(_u(path)).timeout(_timeout);
    if (r.statusCode != 200) {
      throw Exception('GET $path → ${r.statusCode}');
    }
    return jsonDecode(r.body) as Map<String, dynamic>;
  }

  Future<Healthz> health() async => Healthz.fromJson(await _get('/healthz'));
  Future<Account> account() async => Account.fromJson(await _get('/api/v1/paper/account'));
  Future<List<SignalRow>> signals() async {
    final j = await _get('/api/v1/signals');
    return ((j['signals'] as List?) ?? const [])
        .map((e) => SignalRow.fromJson(Map<String, dynamic>.from(e as Map)))
        .toList();
  }
  Future<Performance> performance({int days = 5, int limit = 200}) async {
    final j = await _get('/api/v1/performance?days=$days&limit=$limit');
    return Performance.fromJson(j);
  }

  /// WS 订阅：每个 topic 对应一个 Stream
  Stream<WsEvent> wsStream(List<String> topics) async* {
    final url = baseUrl.replaceFirst('http', 'ws') + '/ws';
    final socket = await WebSocket.connect(url);
    socket.add(jsonEncode({'action': 'subscribe', 'topics': topics}));
    await for (final msg in socket) {
      try {
        final m = jsonDecode(msg as String) as Map<String, dynamic>;
        yield WsEvent(m['topic']?.toString() ?? '?', m['ts']?.toString() ?? '', m['payload']);
      } catch (_) {
        yield WsEvent('?', DateTime.now().toIso8601String(), msg);
      }
    }
  }

  void close() => _http.close();
}