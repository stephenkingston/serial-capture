// Firmware companion to serial-capture's Windows test suite.
//
// Flash this to an Arduino Uno (or compatible CDC-ACM/CH340/FTDI clone),
// then run tests/run_tests.py on Windows. The firmware emits deterministic
// patterns the harness can verify against the captured log.
//
// Commands (terminated by '\n', '\r' is ignored):
//   PING            -> "PONG\r\n"
//   BANNER          -> banner line
//   BURST <n>       -> "BURST:" + n bytes (i & 0xFF)         + "\r\n"
//   BIN <n>         -> n bytes (0x80 | i & 0x7F), 50ms gap, "END\r\n"
//   LARGE           -> 512 bytes of 'A' + "\r\n"
//   <anything else> -> "ECHO:<line>\r\n"

const uint32_t BAUD = 115200;
const char BANNER[] = "READY:serial-capture-test:v1";

char buf[128];
uint8_t len = 0;

void setup() {
  Serial.begin(BAUD);
  while (!Serial) {}        // no-op on Uno; matters on Leonardo/Micro clones
  Serial.println(BANNER);
}

void loop() {
  while (Serial.available()) {
    char c = (char)Serial.read();
    if (c == '\n') {
      buf[len] = 0;
      handle(buf);
      len = 0;
    } else if (c != '\r' && len < sizeof(buf) - 1) {
      buf[len++] = c;
    }
  }
}

void handle(const char* cmd) {
  if (!strcmp(cmd, "PING")) {
    Serial.println("PONG");
  } else if (!strcmp(cmd, "BANNER")) {
    Serial.println(BANNER);
  } else if (!strncmp(cmd, "BURST ", 6)) {
    int n = atoi(cmd + 6);
    if (n < 0) n = 0;
    if (n > 1024) n = 1024;
    Serial.print("BURST:");
    for (int i = 0; i < n; i++) Serial.write((uint8_t)(i & 0xFF));
    Serial.print("\r\n");
  } else if (!strncmp(cmd, "BIN ", 4)) {
    int n = atoi(cmd + 4);
    if (n < 0) n = 0;
    if (n > 256) n = 256;
    for (int i = 0; i < n; i++) Serial.write((uint8_t)(0x80 | (i & 0x7F)));
    Serial.flush();   // drain 328P UART tx buffer
    delay(50);        // let 16U2 USB-CDC flush the binary URB on its own
    Serial.println("END");
  } else if (!strcmp(cmd, "LARGE")) {
    for (int i = 0; i < 512; i++) Serial.write('A');
    Serial.print("\r\n");
  } else {
    Serial.print("ECHO:");
    Serial.println(cmd);
  }
}
