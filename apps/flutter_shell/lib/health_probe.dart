/// 摸金小王子 · Flutter 壳健康探测（可拷进正式工程）
/// 依赖: http

import 'dart:convert';
import 'package:http/http.dart' as http;

Future<Map<String, dynamic>> probeHealth({
  String baseUrl = 'http://127.0.0.1:8787',
}) async {
  final uri = Uri.parse('$baseUrl/healthz');
  final res = await http.get(uri).timeout(const Duration(seconds: 5));
  if (res.statusCode != 200) {
    throw Exception('healthz ${res.statusCode}');
  }
  return jsonDecode(res.body) as Map<String, dynamic>;
}

/// 示例 main（需 Flutter Material）
/*
import 'package:flutter/material.dart';

void main() => runApp(const MojinApp());

class MojinApp extends StatefulWidget {
  const MojinApp({super.key});
  @override
  State<MojinApp> createState() => _MojinAppState();
}

class _MojinAppState extends State<MojinApp> {
  String status = '探测中…';

  @override
  void initState() {
    super.initState();
    probeHealth().then((m) {
      setState(() => status = m['ok'] == true
          ? 'OK · 表${m['tables']} · 权益${m['paper_equity']}'
          : '异常');
    }).catchError((e) => setState(() => status = '失败: $e'));
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      home: Scaffold(
        appBar: AppBar(title: const Text('摸金小王子')),
        body: Center(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(status),
              const SizedBox(height: 12),
              const Text('家庭自用 · 不构成投资建议',
                  style: TextStyle(fontSize: 12, color: Colors.grey)),
            ],
          ),
        ),
      ),
    );
  }
}
*/
