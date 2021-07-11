import 'dart:async';
import 'dart:convert';
import 'dart:html' as html;
import 'package:bson/bson.dart';

Future<void> getBson(html.MouseEvent event) async {
  const path = 'http://localhost:8000/bson';
  try {
    final bsonData = await html.HttpRequest.getString(path);
    // BSON().deserialize(BsonBinary.from(bsonData));
    print("success fetching, but don't know how to deserialize.");
  } catch (e) {
    print("Couldn't open $path");
  }
}

Future<void> getJson(html.MouseEvent event) async {
  const path = 'http://localhost:8000/json/envelope';
  try {
    final jsonData = await html.HttpRequest.getString(path);
    processJson(jsonData);
  } catch (e) {
    print("Couldn't open $path");
  }
}

void processJson(String jsonString) {
  // FIXME: assumes two channels
  var c1 = html.querySelector('#envelope1') as html.CanvasElement?;
  var c2 = html.querySelector('#envelope2') as html.CanvasElement?;
  if (c1 == null || c2 == null) {
    print('Failed to find #envelope1 and #envelope2 in html');
    return;
  }
  var ctx1 = c1.context2D;
  var ctx2 = c2.context2D;
  print(
      'c1 width ${c1.width}, c1 height ${c1.height}, c1 clientWidth ${c1.clientWidth}, c1 clientHeight ${c1.clientHeight}, ');
  ctx1.setFillColorHsl(180, 100, 90);
  ctx2.setFillColorHsl(225, 100, 90);
  ctx1.fillRect(0, 0, 512, 128);
  ctx2.fillRect(0, 0, 512, 128);
  ctx1.setFillColorHsl(190, 100, 90);
  ctx2.setFillColorHsl(235, 100, 90);
  ctx1.fillRect(512, 0, 512, 128);
  ctx2.fillRect(512, 0, 512, 128);
  ctx1.setFillColorHsl(180, 100, 30);
  ctx2.setFillColorHsl(225, 100, 30);

  var xx = 0;
  var l = true;
  for (final envVal in json.decode(jsonString)) {
    if (l) {
      ctx1.fillRect(xx, 64 - 64 * envVal, 1, 128 * envVal);
      l = !l;
    } else {
      ctx2.fillRect(xx, 64 - 64 * envVal, 1, 128 * envVal);
      xx += 1;
      l = !l;
    }
  }
  print(jsonString);
}

Future<void> main() async {
  var getJsonButton = html.querySelector('#getjson');
  if (getJsonButton != null) {
    getJsonButton.onClick.listen(getJson);
  } else {
    print('Failed to find #getjson in html');
  }
}
