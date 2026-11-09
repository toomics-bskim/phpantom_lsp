<?php
// 한글 😀 — spans include the original source, even when braces are implicit.
class LegacyValue { function legacyMember() {} }
class ModernValue { function modernMember() {} }
class DynamicHolder {
    /** @var array{1: ModernValue} */ public $ab;
    /** @var LegacyValue */ public $b;
}
class DynamicCalls {
    /** @var array{1: Closure(): ModernValue} */ public $ab;
    /** @return LegacyValue */ function b() { return new LegacyValue(); }
}
class StaticHolder {
    /** @var array{1: ModernValue} */ public static $ab;
    /** @var LegacyValue */ public static $b;
}
class StaticCalls {
    /** @var array{1: Closure(): ModernValue} */ public static $names;
    /** @return LegacyValue */ static function b() { return new LegacyValue(); }
}
$names = 'ab';
$b = new LegacyValue();
$ab = array(1 => new ModernValue());
$object = new DynamicHolder();
$object->ab = array(1 => new ModernValue());
$object->b = new LegacyValue();
$calls = new DynamicCalls();
$calls->ab = array(1 => function () { return new ModernValue(); });
StaticHolder::$ab = array(1 => new ModernValue());
StaticHolder::$b = new LegacyValue();
StaticCalls::$names = array(1 => function () { return new ModernValue(); });
