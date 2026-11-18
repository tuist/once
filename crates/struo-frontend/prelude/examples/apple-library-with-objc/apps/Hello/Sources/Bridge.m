#import "Bridge.h"

@implementation Bridge
+ (NSString * _Nullable)formatGreeting:(NSString *)name {
    if (name.length == 0) {
        return nil;
    }
    return [NSString stringWithFormat:@"Hello, %@!", name];
}
@end
