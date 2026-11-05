import 'package:flutter_test/flutter_test.dart';
import 'package:survivalpedia/share.dart';

void main() {
  test('Wi-Fi QR text escapes the special characters', () {
    expect(wifiQr('AndroidShare_1234', 'abc123xyz'), 'WIFI:T:WPA;S:AndroidShare_1234;P:abc123xyz;;');
    expect(wifiQr(r'Bob;s "net"', r'p:a,s\s'), r'WIFI:T:WPA;S:Bob\;s \"net\";P:p\:a\,s\\s;;');
    expect(wifiQr('Open', null), 'WIFI:T:nopass;S:Open;;');
  });

  test('network addresses leave out loopback', () async {
    final a = await lanAddresses();
    expect(a, isNot(contains('127.0.0.1')));
  });
}
