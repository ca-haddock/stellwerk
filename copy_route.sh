#!/bin/bash

TABLE=$1


#ip route flush table ${TABLE}

ip route show | grep -v "^default" | grep -E "^(10\.|172\.(1[6-9]|2[0-9]|3[01])\.|192\.168\.)" | while read route; do
    echo  ip route add $route table ${TABLE} 
    ip route add $route table ${TABLE} 
done

echo "Private Netzwerk-Routen wurden in Tabelle ${TABLE} kopiert"
ip route show table ${TABLE}

ip route add default via  172.16.150.7   dev dt6 table 100 
ip route add 172.16.150.0/24 dev dt6 table 100

