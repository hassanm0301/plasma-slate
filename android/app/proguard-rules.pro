# protobuf-javalite uses reflection on generated message fields.
-keep class * extends com.google.protobuf.GeneratedMessageLite { <fields>; }
-dontwarn com.google.protobuf.**
