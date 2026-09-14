import Account
import GraphVendor
#if GRAPH_COMMON && GRAPH_DEBUG && GRAPH_MAC
print(accountValue + Int(graph_vendor_value()))
#elseif GRAPH_COMMON && GRAPH_DEBUG && GRAPH_IOS
print(accountValue + Int(graph_vendor_value()))
#else
#error("Native configuration inheritance was lost")
#endif
