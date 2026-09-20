/// 摸金小王子 · Flutter 壳入口
library;

import 'package:flutter/widgets.dart';

import 'home_screen.dart';

void main() {
  runApp(const MojinApp());
}

class MojinApp extends StatelessWidget {
  const MojinApp({super.key});

  @override
  Widget build(BuildContext context) {
    const baseUrl = String.fromEnvironment('MOJIN_BASE',
        defaultValue: 'http://127.0.0.1:8787');
    return WidgetsApp(
      title: '摸金小王子',
      color: const Color(0xFF0D1117),
      builder: (_, __) => HomeScreen(baseUrl: baseUrl),
      pageRouteBuilder: (ctx, _, __) => PageRouteBuilder(pageBuilder: (c, a, s) => const HomeScreen()),
    );
  }
}